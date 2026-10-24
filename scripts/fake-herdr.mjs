#!/usr/bin/env node
// Fake Herdr JSON API server for manual tests of the Deck bridge.
//   node scripts/fake-herdr.mjs <mode> [path]
// modes: silent (accept, never reply) | slow <ms> | ack-error | close-after-ack | ok
// Point Deck at it with HERDR_SOCKET_PATH=<path> (Windows: a pipe name, e.g. deck-fake;
// the bridge prefixes \\.\pipe\ itself).
import net from "node:net";

const [mode = "ok", arg] = process.argv.slice(2);
const path = process.platform === "win32" ? `\\\\.\\pipe\\${process.env.HERDR_SOCKET_PATH ?? "deck-fake"}` : (process.env.HERDR_SOCKET_PATH ?? "/tmp/deck-fake.sock");
const delay = mode === "slow" ? Number(arg ?? 2000) : 0;

const reply = (req) => {
  const id = req.id;
  if (mode === "ack-error") return { id, error: { message: "fake: refused" } };
  switch (req.method) {
    case "ping": return { id, result: { type: "pong", version: "fake", protocol: 22 } };
    case "workspace.list": return { id, result: { type: "workspace_list", workspaces: [{ workspace_id: "w1", number: 1, label: "fake", focused: true, pane_count: 1, tab_count: 1, active_tab_id: "w1:t1", agent_status: "idle", tokens: {} }] } };
    case "tab.list": return { id, result: { type: "tab_list", tabs: [{ tab_id: "w1:t1", workspace_id: "w1", number: 1, label: "fake", focused: true, pane_count: 1, agent_status: "idle" }] } };
    case "pane.list": return { id, result: { type: "pane_list", panes: [{ pane_id: "w1:p1", terminal_id: "t1", workspace_id: "w1", tab_id: "w1:t1", focused: true, agent: "claude", agent_status: "idle", tokens: {} }] } };
    case "agent.list": return { id, result: { type: "agent_list", agents: [{ terminal_id: "t1", agent: "claude", agent_status: "idle", workspace_id: "w1", tab_id: "w1:t1", pane_id: "w1:p1", focused: true, tokens: {} }] } };
    case "pane.read": return { id, result: { type: "pane_read", read: { pane_id: "w1:p1", workspace_id: "w1", tab_id: "w1:t1", source: "recent", format: "ansi", text: `fake pane ${new Date().toISOString()}\r\n❯ `, revision: 0, truncated: false } } };
    case "events.subscribe": return { id, result: { type: "events_subscribed" } };
    default: return { id, result: { type: "ok" } };
  }
};

const server = net.createServer((sock) => {
  let buf = "";
  sock.on("data", (chunk) => {
    if (mode === "silent") return;
    buf += chunk.toString("utf8");
    let nl;
    while ((nl = buf.indexOf("\n")) >= 0) {
      const line = buf.slice(0, nl); buf = buf.slice(nl + 1);
      if (!line.trim()) continue;
      let req; try { req = JSON.parse(line); } catch { continue; }
      const send = () => { sock.write(JSON.stringify(reply(req)) + "\n"); if (mode === "close-after-ack") sock.end(); };
      delay ? setTimeout(send, delay) : send();
    }
  });
  sock.on("error", () => {});
});
server.listen(path, () => console.log(`fake-herdr (${mode}) listening on ${path}`));
