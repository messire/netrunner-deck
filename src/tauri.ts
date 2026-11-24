/**
 * The Tauri bridge, with a browser fallback. Inside the Tauri window these are
 * the real `invoke` / `listen` / window handle. Opened as a plain page (the
 * Vite dev server in a browser, `npm run dev`), Tauri is absent, so a small
 * in-memory stand-in answers the same commands with synthetic data: enough to
 * look at and drive the UI (menus, palette, rendering) without Herdr. The
 * stand-in never ships behaviour of its own — every command maps to what
 * the Rust side would answer.
 */
import { invoke as tauriInvoke } from "@tauri-apps/api/core";
import { listen as tauriListen, type UnlistenFn } from "@tauri-apps/api/event";
import { getCurrentWindow as tauriWindow } from "@tauri-apps/api/window";

export const inTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

export const invoke: <T>(cmd: string, args?: Record<string, unknown>) => Promise<T> =
  inTauri ? tauriInvoke : (cmd, args) => fake.invoke(cmd, args ?? {}) as Promise<any>;

export const listen: <T>(event: string, cb: (e: { payload: T }) => void) => Promise<UnlistenFn> =
  inTauri ? tauriListen : (event, cb) => fake.listen(event, cb as (e: { payload: unknown }) => void);

export const getCurrentWindow = inTauri
  ? tauriWindow
  : () => ({ minimize: async () => {}, toggleMaximize: async () => {}, close: async () => { window.close(); } });

// ---------------------------------------------------------------------------
// Browser stand-in. Deterministic, mutable through the same actions the UI
// offers, with a pane that keeps "typing" so incremental rendering is visible.
// ---------------------------------------------------------------------------
type Listener = (e: { payload: unknown }) => void;

const fake = (() => {
  const listeners = new Map<string, Set<Listener>>();
  const emit = (event: string, payload: unknown) => listeners.get(event)?.forEach((l) => l({ payload }));

  let n = 0;
  const id = (p: string) => `${p}${++n}`;
  const workspaces = [
    { workspace_id: "w1", number: 1, label: "Atlas", focused: true, pane_count: 4, tab_count: 3, active_tab_id: "w1:t1", agent_status: "working", worktree: { repo_key: "atlas", repo_name: "Atlas", is_linked_worktree: false } },
    { workspace_id: "w2", number: 2, label: "Infra", focused: false, pane_count: 1, tab_count: 1, active_tab_id: "w2:t1", agent_status: "blocked" },
    { workspace_id: "w3", number: 3, label: "search-v2", focused: false, pane_count: 1, tab_count: 1, active_tab_id: "w3:t1", agent_status: "idle", worktree: { repo_key: "atlas", repo_name: "Atlas", branch: "search-v2", is_linked_worktree: true } },
  ];
  const tabs = [
    { tab_id: "w1:t1", workspace_id: "w1", number: 1, label: "main", focused: true, pane_count: 1, agent_status: "working" },
    { tab_id: "w1:t2", workspace_id: "w1", number: 2, label: "tests", focused: false, pane_count: 1, agent_status: "blocked" },
    { tab_id: "w1:t3", workspace_id: "w1", number: 3, label: "review", focused: false, pane_count: 2, agent_status: "idle" },
    { tab_id: "w2:t1", workspace_id: "w2", number: 1, label: "ops", focused: true, pane_count: 1, agent_status: "blocked" },
    { tab_id: "w3:t1", workspace_id: "w3", number: 1, label: "1", focused: true, pane_count: 1, agent_status: "idle" },
  ];
  const panes = [
    { pane_id: "w1:p1", terminal_id: "t1", workspace_id: "w1", tab_id: "w1:t1", focused: true, cwd: "C:\\Workspace\\Atlas", agent: "claude", display_agent: "Claude", title: "claude", agent_status: "working", tokens: { ctx: "42" } },
    { pane_id: "w1:p2", terminal_id: "t2", workspace_id: "w1", tab_id: "w1:t2", focused: false, cwd: "C:\\Workspace\\Atlas", agent: "claude", agent_status: "blocked", tokens: { ctx: "71" } },
    { pane_id: "w1:p3", terminal_id: "t5", workspace_id: "w1", tab_id: "w1:t3", focused: false, cwd: "C:\\Workspace\\Atlas", agent: "codex", display_agent: "Codex", title: "codex", agent_status: "idle", tokens: { ctx: "9" } },
    { pane_id: "w1:p4", terminal_id: "t6", workspace_id: "w1", tab_id: "w1:t3", focused: false, cwd: "C:\\Workspace\\Atlas", agent: null, title: "pwsh", agent_status: "unknown" },
    { pane_id: "w2:p1", terminal_id: "t3", workspace_id: "w2", tab_id: "w2:t1", focused: true, cwd: "C:\\Workspace\\Infra", agent: "claude", agent_status: "blocked" },
    { pane_id: "w3:p1", terminal_id: "t4", workspace_id: "w3", tab_id: "w3:t1", focused: true, cwd: "C:\\Workspace\\Atlas-search", agent: null, title: "pwsh", agent_status: "unknown" },
  ];
  const agents = () => panes.filter((p) => p.agent).map((p) => ({ terminal_id: p.terminal_id, agent: p.agent, display_agent: p.display_agent, agent_status: p.agent_status, tokens: p.tokens, workspace_id: p.workspace_id, tab_id: p.tab_id, pane_id: p.pane_id, focused: p.focused }));
  const accounts = {
    activeAccountNumber: 1,
    accounts: [
      { number: 1, email: "one@example.com", alias: "MAIN", active: true, usage: { fiveHour: { pct: 7, countdown: "14m" }, sevenDay: { pct: 4, countdown: "6d 5h" } } },
      { number: 2, email: "two@example.com", alias: "SPARE", active: false, usage: { fiveHour: { pct: 4 }, sevenDay: { pct: 24 } } },
    ],
  };

  // A Claude Code-like transcript that grows a line every few seconds.
  const DIM = "\x1b[2m", RST = "\x1b[0m", BAND = "\x1b[48;2;40;40;40m";
  const base = [
    `${BAND}❯ check the project state${RST}`,
    "",
    "● I'll look at the roadmap and the open items.",
    "",
    "● Read(docs/plan/ROADMAP.md)",
    `  ⎿  ${DIM}Read 230 lines${RST}`,
    "",
    "● M1 is closed; M2 is the next milestone.",
  ];
  const extra = ["  The pane view polls pane.read today.", "  Streaming is planned through the fork.", "  Scroll position should survive updates.", "", "● Done."];
  let grown = 0;
  let revision = 2;
  const transcript = () => {
    const body = [...base, ...extra.slice(0, grown)];
    const box = ["─".repeat(80), `❯ ${"typed".slice(0, grown % 6)}`, "─".repeat(80), `  ${DIM}? for shortcuts${RST}`];
    const lines = [...body, "", ...box];
    const promptIdx = lines.length - 3;
    return { text: lines.join("\n"), revision, cursor: { line: promptIdx, col: 2 + ("typed".slice(0, grown % 6)).length, visible: true, shape: 0 } };
  };
  setInterval(() => {
    if (grown < extra.length) { grown++; revision += 2; emit("herdr://event", { ...(legacy ? {} : { server: "herdr" }), event: "pane_output_changed", data: { type: "pane_output_changed", pane_id: "w1:p1", workspace_id: "w1", revision } }); }
  }, 3000);

  (window as any).__deckEmit = emit;
  const tree = () => ({ workspaces, tabs, panes, agents: agents() });

  // Servers: the user's Herdr and a trial deck-server are connected, a dev
  // session is found but not connected. The second server reuses the same
  // local ids (`w1:p1`), as real servers do. `?servers=0` starts with none
  // connected (the first-run list), `?servers=down` takes deck-server offline.
  const q = new URLSearchParams(location.search);
  // `?servers=legacy`: a Rust side from before SERVERS (no servers_* commands, no `server` on events).
  const legacy = q.get("servers") === "legacy";
  type FakeServer = { id: string; name: string; kind: "herdr" | "deck-server"; address: string; connected: boolean; manual: boolean; found: boolean; new: boolean; status: { state: string; version?: string; protocol?: number; error?: string } };
  const online = { state: "online", version: "0.9.5", protocol: 22 };
  const servers: FakeServer[] = [
    { id: "herdr", name: "herdr", kind: "herdr", address: "\\\\.\\pipe\\C:\\Users\\me\\AppData\\Roaming\\herdr\\herdr.sock", connected: q.get("servers") !== "0", manual: false, found: true, new: false, status: { ...online } },
    { id: "deck-server/try", name: "deck-server · try", kind: "deck-server", address: "\\\\.\\pipe\\C:\\Users\\me\\AppData\\Roaming\\deck-server\\sessions\\try\\herdr.sock", connected: q.get("servers") !== "0", manual: false, found: true, new: false,
      status: q.get("servers") === "down" ? { state: "offline", error: "herdr server is not reachable at \\\\.\\pipe\\…\\try\\herdr.sock: Access is denied. (os error 5) — Herdr runs elevated" } : { ...online } },
    { id: "herdr-dev/decktest", name: "herdr-dev · decktest", kind: "herdr", address: "\\\\.\\pipe\\C:\\Users\\me\\AppData\\Roaming\\herdr-dev\\sessions\\decktest\\herdr.sock", connected: false, manual: false, found: true, new: true, status: { ...online } },
  ];
  const view = () => ({ servers: servers.filter((s) => s.connected || s.found || s.manual).map((s) => ({ ...s })), scanned: true });
  const changed = () => { setTimeout(() => emit("deck://servers", "changed"), 5); return view(); };
  const srv = (id: unknown) => { const s = servers.find((x) => x.id === id); if (!s) throw new Error(`unknown server ${String(id)}`); return s; };
  const trial = {
    workspaces: [{ workspace_id: "w1", number: 1, label: "scratch", focused: true, pane_count: 2, tab_count: 1, active_tab_id: "w1:t1", agent_status: "idle" }],
    tabs: [{ tab_id: "w1:t1", workspace_id: "w1", number: 1, label: "scratch", focused: true, pane_count: 2, agent_status: "idle" }],
    panes: [
      { pane_id: "w1:p1", terminal_id: "t1", workspace_id: "w1", tab_id: "w1:t1", focused: true, cwd: "C:\\Temp\\scratch", agent: null, title: "pwsh", agent_status: "unknown" },
      { pane_id: "w1:p2", terminal_id: "t2", workspace_id: "w1", tab_id: "w1:t1", focused: false, cwd: "C:\\Temp\\scratch", agent: "deck-claude", agent_status: "blocked" },
    ],
    agents: [{ terminal_id: "t2", agent: "deck-claude", agent_status: "blocked", workspace_id: "w1", tab_id: "w1:t1", pane_id: "w1:p2", focused: false }],
  };
  const treeOf = (server: unknown) => {
    const s = srv(server ?? "herdr");
    if (!s.connected) throw new Error(`server ${s.id} is not connected`);
    if (s.status.state === "offline") throw new Error(s.status.error ?? "offline");
    return s.id === "herdr" ? tree() : s.id === "deck-server/try" ? trial : { workspaces: [], tabs: [], panes: [], agents: [] };
  };
  // A headless Claude session (deck-server's `deck-server agent`) on the trial
  // server's w1:p2: it asks a question, then wants to write a file, and
  // answers each message with streamed text.
  const agentLine = (msg: Record<string, unknown>) => ({ event: "deck.agent.event", data: { pane_id: "w1:p2", epoch: "fake", seq: ++agentSeq, msg } });
  let agentSeq = 0;
  const agentEmit = (msg: Record<string, unknown>) => emit("deck://agent", { server: "deck-server/try", pane_id: "w1:p2", line: agentLine(msg) });
  const agentAsk = (id: string, tool: string, input: Record<string, unknown>, extra: Record<string, unknown> = {}) =>
    agentEmit({ type: "control_request", request_id: id, request: { subtype: "can_use_tool", tool_name: tool, input, tool_use_id: `tu-${id}`, ...extra } });
  const agentSay = async (text: string) => {
    agentEmit({ type: "stream_event", event: { type: "message_start" } });
    agentEmit({ type: "stream_event", event: { type: "content_block_start", content_block: { type: "text" } } });
    for (const word of text.split(/(?<= )/)) { await new Promise((r) => setTimeout(r, 60)); agentEmit({ type: "stream_event", event: { type: "content_block_delta", delta: { type: "text_delta", text: word } } }); }
  };
  const agentStart = () => {
    emit("deck://agent", { server: "deck-server/try", pane_id: "w1:p2", line: { id: "s", result: { pane_id: "w1:p2", epoch: "fake", connected: true, status: "blocked", session_id: "fake-session" } } });
    agentEmit({ type: "control_response", response: { subtype: "success", request_id: "deck-init", response: {
      commands: [{ name: "compact", description: "Clear history but keep a summary", argumentHint: "" }, { name: "review", description: "Review a pull request", argumentHint: "<pr>" }, { name: "init", description: "Create a CLAUDE.md", argumentHint: "" }],
      models: [{ value: "default", displayName: "Default (recommended)", description: "Opus for complex tasks" }, { value: "sonnet", displayName: "Sonnet", description: "Fast for everyday work" }, { value: "haiku", displayName: "Haiku", description: "Fastest" }] } } });
    agentEmit({ type: "system", subtype: "init", session_id: "fake-session", model: "claude-opus-5-5", permissionMode: "default" });
    agentEmit({ type: "user", message: { role: "user", content: "pick a file and write hi into it" } });
    void agentSay("I'll ask which file you want first. ").then(() => agentAsk("q1", "AskUserQuestion", { questions: [{ question: "Which file should I write into?", header: "File", multiSelect: false, options: [{ label: "a.txt", description: "next to the README" }, { label: "b.txt", description: "in the scratch folder" }] }] }));
  };
  const agentCall = async (method: string, params: Record<string, unknown>) => {
    console.debug("[stand-in] agent", method, JSON.stringify(params).slice(0, 200));
    if (method === "respond") {
      const id = String(params.request_id);
      agentEmit({ type: "deck_resolved", request_id: id });
      if (id === "q1") {
        await agentSay("Writing it now. ");
        agentAsk("w1", "Write", { file_path: "C:\\Temp\\scratch\\a.txt", content: "hi\n" }, { description: "a.txt", permission_suggestions: [{ type: "setMode", mode: "acceptEdits", destination: "session" }] });
      } else {
        await agentSay("Done: the file has **hi** in it.");
        agentEmit({ type: "result", subtype: "success", is_error: false, result: "Done" });
      }
    } else if (method === "send") {
      agentEmit({ type: "user", message: { role: "user", content: String(params.text) } });
      await agentSay("Here is a streamed answer to your message, word by word, the way the headless session writes it. ");
      agentEmit({ type: "result", subtype: "success", is_error: false, result: "ok" });
    } else if (method === "set") {
      agentEmit({ type: "deck_settings", ...(params.permission_mode ? { permission_mode: params.permission_mode } : {}), ...("model" in params ? { model: params.model } : {}) });
    } else if (method === "interrupt") {
      agentEmit({ type: "result", subtype: "error_during_execution", is_error: true, result: "Interrupted by the user" });
    }
    return { queued: false };
  };
  const invoke = async (cmd: string, args: Record<string, unknown>): Promise<unknown> => {
    await new Promise((r) => setTimeout(r, 30));
    switch (cmd) {
      case "agent_watch": if (args.server === "deck-server/try" && args.paneId === "w1:p2") setTimeout(agentStart, 200); return null;
      case "agent_unwatch": return null;
      case "agent_call": return agentCall(String(args.method), (args.params ?? {}) as Record<string, unknown>);
      case "transcript_read": return { session_id: args.sessionId, path: "", start: 0, end: 0, len: 0, lines: [] };
      case "transcript_watch": case "transcript_mtimes": return cmd === "transcript_mtimes" ? {} : null;
      case "herdr_ping": return { version: "browser-fake", protocol: 0 };
      case "herdr_tree": return treeOf(legacy ? null : args.server);
      case "servers_list": case "servers_seen":
        if (legacy) throw new Error(`Command ${cmd} not found`);
        if (cmd === "servers_seen") servers.forEach((s) => (s.new = false));
        return view();
      case "servers_scan": await new Promise((r) => setTimeout(r, 400)); return view();
      case "server_connect": { const s = srv(args.id); s.connected = true; return changed(); }
      case "server_disconnect": { const s = srv(args.id); s.connected = false; s.new = false; return changed(); }
      case "server_rename": { const s = srv(args.id); s.name = String(args.name).trim() || s.id; return changed(); }
      case "server_forget": { const i = servers.findIndex((x) => x.id === args.id); if (i >= 0) servers.splice(i, 1); return changed(); }
      case "server_add": {
        const path = String(args.address).trim().replace(/^\\\\\.\\pipe\\/, "");
        if (!servers.some((x) => x.id === path)) servers.push({ id: path, name: path, kind: "herdr", address: `\\\\.\\pipe\\${path}`, connected: true, manual: true, found: false, new: false, status: { state: "offline", error: `herdr server is not reachable at \\\\.\\pipe\\${path}: The system cannot find the file specified. (os error 2) — is herdr server running?` } });
        return changed();
      }
      case "herdr_pane_read": {
        const paneId = String(args.paneId);
        // Test hook: a page script may script the snapshots (`window.__deckPaneRead`)
        // and raise events (`window.__deckEmit`) to drive the pane view.
        const hook = (window as any).__deckPaneRead as ((paneId: string, lines: number) => unknown) | undefined;
        if (hook) return { read: hook(paneId, Number(args.lines)) };
        if (args.server === "deck-server/try") return { read: { text: `PS C:\\Temp\\scratch> # deck-server · ${paneId}\nPS C:\\Temp\\scratch> `, revision: 0 } };
        if (paneId === "w1:p1") return { read: transcript() };
        return { read: { text: `PS ${panes.find((p) => p.pane_id === paneId)?.cwd ?? "C:\\"}> echo ${paneId}\n${paneId}\nPS> `, revision: 0 } };
      }
      case "herdr_pane_send_text": case "herdr_pane_send_keys": console.debug("[stand-in]", cmd, JSON.stringify(args)); return null;
      case "herdr_focus": return null;
      case "clipboard_read_text": return navigator.clipboard.readText().catch(() => "");
      case "clipboard_write_text": return navigator.clipboard.writeText(String(args.text)).catch(() => undefined);
      case "diag_log": console.warn("[diag]", args.line); return null;
      case "herdr_action": {
        const action = String(args.action), target = args.id as string | null;
        if (action === "workspace.create") {
          const w = id("w"); workspaces.push({ workspace_id: w, number: workspaces.length + 1, label: `new-${w}`, focused: false, pane_count: 1, tab_count: 1, active_tab_id: `${w}:t1`, agent_status: "unknown" });
          tabs.push({ tab_id: `${w}:t1`, workspace_id: w, number: 1, label: "1", focused: true, pane_count: 1, agent_status: "unknown" });
          panes.push({ pane_id: `${w}:p1`, terminal_id: id("t"), workspace_id: w, tab_id: `${w}:t1`, focused: true, cwd: "C:\\", agent: null, title: "pwsh", agent_status: "unknown" });
        } else if (action === "tab.create" && target) {
          const t = `${target}:${id("t")}`; tabs.push({ tab_id: t, workspace_id: target, number: tabs.length + 1, label: String(tabs.length + 1), focused: false, pane_count: 1, agent_status: "unknown" });
          panes.push({ pane_id: `${t}:p1`, terminal_id: id("t"), workspace_id: target, tab_id: t, focused: false, cwd: "C:\\", agent: null, title: "pwsh", agent_status: "unknown" });
        } else if (action === "tab.close" && target) {
          for (let i = panes.length - 1; i >= 0; i--) if (panes[i].tab_id === target) panes.splice(i, 1);
          const i = tabs.findIndex((t) => t.tab_id === target); if (i >= 0) tabs.splice(i, 1);
        } else if (action === "pane.close" && target) {
          const i = panes.findIndex((p) => p.pane_id === target); if (i >= 0) panes.splice(i, 1);
        } else if (action === "pane.split" && target) {
          const src = panes.find((p) => p.pane_id === target);
          if (src) {
            panes.push({ pane_id: `${src.tab_id}:${id("p")}`, terminal_id: id("t"), workspace_id: src.workspace_id, tab_id: src.tab_id, focused: false, cwd: src.cwd, agent: null, title: "pwsh", agent_status: "unknown" });
            const tb = tabs.find((t) => t.tab_id === src.tab_id); if (tb) tb.pane_count++;
          }
        } else if (action === "workspace.close" && target) {
          const w = workspaces.find((x) => x.workspace_id === target);
          if (w && !w.worktree && workspaces.some((x) => x.worktree?.repo_name === w.label) && !(args.options as any)?.close_group) throw new Error("herdr returned an error for workspace.close: workspace_group_close_required");
          for (let i = panes.length - 1; i >= 0; i--) if (panes[i].workspace_id === target) panes.splice(i, 1);
          for (let i = tabs.length - 1; i >= 0; i--) if (tabs[i].workspace_id === target) tabs.splice(i, 1);
          const i = workspaces.findIndex((x) => x.workspace_id === target); if (i >= 0) workspaces.splice(i, 1);
        }
        setTimeout(() => emit("herdr://event", { server: String(args.server ?? "herdr"), event: "layout_updated" }), 10);
        return null;
      }
      case "notify": console.info("[notify]", args.title, "—", args.body); return null;
      // `?update=1` in the browser shows the update offer; install just waits a bit.
      case "update_check":
        return new URLSearchParams(location.search).has("update")
          ? { version: "9.9.9", current: "0.1.0", notes: "browser stand-in", date: null } : null;
      case "update_install": await new Promise((r) => setTimeout(r, 1500)); return null;
      case "cswap_list": return accounts;
      case "cswap_switch": {
        const n = Number(args.number); accounts.activeAccountNumber = n;
        for (const a of accounts.accounts) a.active = a.number === n;
        return accounts;
      }
      default: throw new Error(`browser stand-in: unknown command ${cmd}`);
    }
  };
  const listen = async (event: string, cb: Listener): Promise<UnlistenFn> => {
    if (!listeners.has(event)) listeners.set(event, new Set());
    listeners.get(event)!.add(cb);
    return () => { listeners.get(event)?.delete(cb); };
  };
  return { invoke, listen };
})();
