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
  const noLogin = new URLSearchParams(location.search).has("signin");
  const accounts: { activeAccountNumber: number | null; accounts: { number: number; email: string; alias?: string; active: boolean; usageStatus?: string; usage?: Record<string, unknown> }[] } = noLogin
    ? { activeAccountNumber: null, accounts: [] }
    : {
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
  type FakeServer = { id: string; name: string; kind: "herdr" | "deck-server"; address: string; connected: boolean; manual: boolean; found: boolean; new: boolean; remote?: boolean; machine?: string | null; status: { state: string; version?: string; protocol?: number; error?: string } };
  const online = { state: "online", version: "0.9.5", protocol: 22 };
  const servers: FakeServer[] = [
    { id: "herdr", name: "herdr", kind: "herdr", address: "\\\\.\\pipe\\C:\\Users\\me\\AppData\\Roaming\\herdr\\herdr.sock", connected: q.get("servers") !== "0", manual: false, found: true, new: false, status: { ...online } },
    { id: "deck-server/try", name: "deck-server · try", kind: "deck-server", remote: q.get("remote") === "1", address: "\\\\.\\pipe\\C:\\Users\\me\\AppData\\Roaming\\deck-server\\sessions\\try\\herdr.sock", connected: q.get("servers") !== "0", manual: false, found: true, new: false,
      status: q.get("servers") === "down" ? { state: "offline", error: "herdr server is not reachable at \\\\.\\pipe\\…\\try\\herdr.sock: Access is denied. (os error 5) — Herdr runs elevated" } : { ...online } },
    { id: "herdr-dev/decktest", name: "herdr-dev · decktest", kind: "herdr", address: "\\\\.\\pipe\\C:\\Users\\me\\AppData\\Roaming\\herdr-dev\\sessions\\decktest\\herdr.sock", connected: false, manual: false, found: true, new: true, status: { ...online } },
    { id: "dev@build-box.example.com/deck-server", name: "deck-server @ dev@build-box.example.com", kind: "deck-server", address: "dev@build-box.example.com:/home/dev/.config/deck-server/herdr.sock", connected: q.get("xfer") === "1", manual: true, found: true, new: false, remote: true, machine: "dev@build-box.example.com", status: { state: "online", version: "0.4.0", protocol: 22 } },
  ];
  // Saved machines the last scan could not reach (`?servers=0` has none).
  const unreachable: { machine: string; error: string }[] = q.get("servers") === "0" ? [] : [{ machine: "dev@old-box.example.com:2222", error: "ssh dev@old-box.example.com:2222: no answer within 20s" }];
  const view = () => ({ servers: servers.filter((s) => s.connected || s.found || s.manual).map((s) => ({ ...s })), scanned: true, unreachable: unreachable.map((m) => ({ ...m })) });
  /** CHECK in the ADD dialog: a host with `down`, `nobridge`, `refused` or `empty` in its name fails that way. */
  const probe = (user: string, host: string, port: number) => {
    const key = `${user}@${host.includes(":") ? `[${host}]` : host}${port === 22 ? "" : `:${port}`}`;
    if (/down/.test(host)) throw new Error(`ssh ${key}: no answer within 20s`);
    if (/refused/.test(host)) throw new Error(`ssh ${key}: ${user}@${host}: Permission denied (publickey).`);
    if (/nobridge/.test(host)) throw new Error(`bridge on ${key}: no deck-server with a bridge there`);
    if (/empty/.test(host)) throw new Error(`no Herdr or deck-server is running on ${key}`);
    const home = `/home/${user}/.config`;
    return [
      { id: `${key}/herdr`, name: "herdr", kind: "herdr" as const, socket: `${home}/herdr/herdr.sock`, version: "0.9.5", protocol: 22, preferred: false },
      { id: `${key}/deck-server`, name: "deck-server", kind: "deck-server" as const, socket: `${home}/deck-server/herdr.sock`, version: "0.4.0", protocol: 22, preferred: true },
      { id: `${key}/deck-server/try`, name: "deck-server · try", kind: "deck-server" as const, socket: `${home}/deck-server/sessions/try/herdr.sock`, version: "0.4.0", protocol: 22, preferred: false },
    ].map((p) => ({ ...p, connected: !!servers.find((s) => s.id === p.id)?.connected, key }));
  };
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
  if (q.get("xfer") === "1") {
    trial.workspaces = [
      { workspace_id: "w1", number: 1, label: "atlas", focused: true, pane_count: 2, tab_count: 1, active_tab_id: "w1:t1", agent_status: "idle", worktree: { repo_key: "atlas", repo_name: "atlas", branch: "main", is_linked_worktree: false, checkout_path: "C:\\Temp\\atlas" } },
      { workspace_id: "w2", number: 2, label: "feat", focused: false, pane_count: 1, tab_count: 1, active_tab_id: "w2:t1", agent_status: "idle", worktree: { repo_key: "atlas", repo_name: "atlas", branch: "feat/clone", is_linked_worktree: true, checkout_path: "C:\\Temp\\atlas-feat" } },
    ] as never;
    trial.tabs.push({ tab_id: "w2:t1", workspace_id: "w2", number: 1, label: "feat chat", focused: false, pane_count: 1, agent_status: "idle" });
    trial.panes.push({ pane_id: "w2:p1", terminal_id: "t3", workspace_id: "w2", tab_id: "w2:t1", focused: false, cwd: "C:\\Temp\\atlas-feat", agent: "deck-claude", agent_status: "idle" });
  }
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
      case "server_stop": { const s = srv(args.id); s.connected = false; s.status = { state: "offline" } as typeof s.status; return changed(); }
      case "server_rename": { const s = srv(args.id); s.name = String(args.name).trim() || s.id; return changed(); }
      case "server_probe": { await new Promise((r) => setTimeout(r, 900)); return probe(String(args.user), String(args.host), Number(args.port)).map(({ key: _k, ...p }) => p); }
      case "server_add_host": {
        const found = probe(String(args.user), String(args.host), Number(args.port));
        const sockets = args.sockets as string[];
        for (const p of found) {
          const s = servers.find((x) => x.id === p.id);
          const on = sockets.includes(p.socket);
          if (s) { s.connected ||= on; s.found = true; continue; }
          servers.push({ id: p.id, name: `${p.name} @ ${p.key}`, kind: p.kind, address: `${p.key}:${p.socket}`, connected: on, manual: on, found: true, new: false, remote: true, machine: p.key, status: { state: "online", version: p.version, protocol: p.protocol } });
        }
        return changed();
      }
      case "server_remove_host": {
        for (let i = servers.length - 1; i >= 0; i--) if (servers[i].machine === args.machine) servers.splice(i, 1);
        const i = unreachable.findIndex((m) => m.machine === args.machine); if (i >= 0) unreachable.splice(i, 1);
        return changed();
      }
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
          if (w && !w.worktree && workspaces.some((x) => x.worktree?.repo_name === w.label) && !(args.options as any)?.close_group) throw new Error("the server refused workspace.close: workspace_group_close_required");
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
      case "fs_call": {
        // A deck-server on another machine keeps folders in its user's home (/home/me).
        const typed = String(args.path).trim();
        const path = typed.startsWith("/home/me") ? typed : `/home/me/${typed.replace(/^~/, "").replace(/^[\/]+/, "")}`.replace(/\/$/, "");
        if (/(^|\/)\.\.(\/|$)/.test(typed)) throw new Error(`the server refused deck.fs.stat: ${typed} leads outside your home folder /home/me`);
        const made = ((window as any).__deckMade ??= new Set<string>()) as Set<string>;
        if (args.method === "mkdir") made.add(path);
        const exists = made.has(path) || path === "/home/me/notes";
        return { path, exists, dir: exists, home: "/home/me" };
      }
      case "repos_list": return { folders: [{ name: "atlas", root: "/home/me/work/atlas", git: true }, { name: "notes", root: "/home/me/notes", git: false }] };
      // deck-server's `deck.accounts.*`; `?signin=1` starts with a machine that has no login.
      case "accounts_call": {
        const p = (args.params ?? {}) as Record<string, unknown>;
        const list = () => ({ ...accounts, managed: true, signedIn: accounts.activeAccountNumber !== null });
        switch (String(args.method)) {
          case "list": return list();
          case "switch": {
            const n = Number(p.number); accounts.activeAccountNumber = n;
            for (const a of accounts.accounts) a.active = a.number === n;
            return list();
          }
          case "remove": accounts.accounts = accounts.accounts.filter((a) => a.number !== Number(p.number)); return list();
          case "login.start": return { login_id: "stand-in", url: "https://claude.com/cai/oauth/authorize?code=true&state=stand-in" };
          case "login.finish": {
            if (!String(p.code ?? "").trim() || String(p.code).includes("bad")) throw new Error("the code was not accepted (it is single-use and expires quickly): start the sign-in again");
            const n = Math.max(0, ...accounts.accounts.map((a) => a.number)) + 1;
            const first = accounts.activeAccountNumber === null || p.activate === true;
            accounts.accounts.push({ number: n, email: `new${n}@example.com`, active: first, usageStatus: "ok", usage: { fiveHour: { pct: 0 }, sevenDay: { pct: 0 } } });
            if (first) { accounts.activeAccountNumber = n; for (const a of accounts.accounts) a.active = a.number === n; }
            return { ...list(), added: n };
          }
          default: throw new Error(`unknown accounts method ${args.method}`);
        }
      }
      // `?xfer=1`: clone, GitHub sign-in and COPY TO / MOVE TO answer like a deck-server would.
      case "machine_call": {
        const m = String(args.method), p = (args.params ?? {}) as Record<string, unknown>;
        const jobs = ((window as any).__deckJobs ??= new Map<string, { kind: string; at: number; extra: Record<string, unknown> }>()) as Map<string, { kind: string; at: number; extra: Record<string, unknown> }>;
        const gh = (window as any).__deckGitHub ??= { signedIn: false, polls: 0 };
        const job = (kind: string, extra: Record<string, unknown> = {}) => { const id = `${kind}-${jobs.size + 1}`; jobs.set(id, { kind, at: Date.now(), extra }); return id; };
        const age = (id: string) => Date.now() - (jobs.get(id)?.at ?? 0);
        switch (m) {
          case "deck.git.clone.start": {
            const path = String(p.path).replace(/^~/, "/home/me");
            if ((window as any).__deckMade?.has(path) || path === "/home/me/notes") throw new Error(`the server refused deck.git.clone.start: ${path} is not empty — pick another folder (a clone goes into a new or empty one)`);
            return { job: job("clone", { url: p.url, path }), path };
          }
          case "deck.git.clone.status": case "deck.git.clone.cancel": {
            const j = jobs.get(String(p.job))!; const t = age(String(p.job));
            if (m.endsWith("cancel")) j.extra.cancelled = true;
            if (j.extra.cancelled) return { state: "cancelled", phase: "", percent: null, message: "cancelled" };
            if (/private/.test(String(j.extra.url)) && !gh.signedIn) return t < 800 ? { state: "running", phase: "Cloning", percent: null } : { state: "failed", phase: "Cloning", message: "remote: Repository not found.\nfatal: repository 'https://github.com/o/private.git/' not found", auth: true, github: true, path: j.extra.path };
            const phases: [string, number][] = [["Counting objects", 100], ["Receiving objects", 37], ["Receiving objects", 81], ["Resolving deltas", 64]];
            const i = Math.floor(t / 700);
            return i < phases.length ? { state: "running", phase: phases[i][0], percent: phases[i][1], path: j.extra.path } : { state: "done", phase: "Resolving deltas", percent: 100, path: j.extra.path };
          }
          case "deck.server.info": return args.server === "deck-server/try" ? { version: "0.9.5", commit: null, updates: false } : { version: "0.10.0", commit: "70314e7295e0", updates: true };
          case "deck.secrets.read": {
            const env = ((window as any).__deckEnv ??= new Map<string, string>([["deck-server/try", "# fake values\nFAKE_TOKEN=not-a-secret\nexport FAKE_URL=\"http://example.invalid/a b\"\n"]])) as Map<string, string>;
            const text = env.get(String(args.server));
            return { path: "/home/me/.claude/.env", exists: text != null, text: text ?? "" };
          }
          case "deck.secrets.write": ((window as any).__deckEnv as Map<string, string>).set(String(args.server), String(p.text)); return { path: "/home/me/.claude/.env" };
          case "deck.github.status": return { available: true, signed_in: gh.signedIn, user: gh.signedIn ? "octo" : null };
          case "deck.github.logout": gh.signedIn = false; return { available: true, signed_in: false, user: null };
          case "deck.github.login.start": gh.polls = 0; return { login_id: "gh-1" };
          case "deck.github.login.status": {
            gh.polls++;
            if (gh.polls < 2) return { state: "installing" };
            if (gh.polls < 5) return { state: "pending", user_code: "938E-09BA", verification_uri: "https://github.com/login/device" };
            gh.signedIn = true;
            return { state: "done", user: "octo", user_code: "938E-09BA", verification_uri: "https://github.com/login/device" };
          }
          case "deck.transfer.export.start": return { job: job("export", { params: p }) };
          case "deck.transfer.import.start": {
            if (p.kind === "repo" && String(p.path) === "~/notes") throw new Error("the server refused deck.transfer.import.start: /home/me/notes is already taken on this machine — pick another name");
            return { job: job("import", { kind: p.kind, path: p.kind === "repo" ? String(p.path).replace(/^~/, "/home/me") : null }), path: p.kind === "repo" ? String(p.path).replace(/^~/, "/home/me") : null };
          }
          case "deck.transfer.import.commit": { const j = jobs.get(String(p.job))!; j.at = Date.now(); j.extra.committed = true; return { job: p.job }; }
          case "deck.transfer.remove.start": return { job: job("remove") };
          case "deck.transfer.conflicts": return { existing: ((p.items as string[]) ?? []).filter((i) => i === "CLAUDE.md") };
          case "deck.transfer.finish": case "deck.transfer.cancel": return {};
          case "deck.transfer.status": {
            const j = jobs.get(String(p.job))!; const t = age(String(p.job));
            if (t < 1200) return { state: "running", phase: j.kind === "export" ? "Packing history" : j.kind === "import" ? "Checking out main" : "Removing", percent: Math.round(t / 12) };
            if (j.kind === "export") return { state: "done", phase: "Packing sessions", percent: 100, size: 3_400_000 };
            if (j.kind === "import" && j.extra.kind === "harness") return { state: "done", percent: 100, copied: ["skills/deploy"], skipped: ["CLAUDE.md"], renamed: [] };
            if (j.kind === "import") return { state: "done", percent: 100, path: j.extra.path, worktrees: [{ path: j.extra.path, branch: "main", root: true }], sessions: [{ id: "fake-session", cwd: j.extra.path }] };
            return { state: "done", percent: 100 };
          }
          default: throw new Error(`the server refused ${m}: unknown method`);
        }
      }
      case "transfer_relay": (window as any).__deckRelay = Date.now(); return { relay: "relay-1" };
      case "transfer_relay_status": { const t = Date.now() - (window as any).__deckRelay; const size = 3_400_000; return { sent: Math.min(size, Math.round(size * t / 2000)), size, done: t >= 2000, error: null }; }
      case "transfer_open": console.info("[stand-in] transfer_open", JSON.stringify(args)); return { resumed: (args.sessions as unknown[]).length };
      case "open_external": console.info("[stand-in] open", args.url); return null;
      case "harness_scan": return [{ key: "personal", name: "Personal", root: "C:/Users/me/.claude", personal: true, exists: true, items: [
        { kind: "claude_md", name: "CLAUDE.md", path: "C:/Users/me/.claude/CLAUDE.md", size: 1200, mtime_ms: Date.now(), local: false, tags: [] },
        { kind: "settings", name: "settings.json", path: "C:/Users/me/.claude/settings.json", size: 300, mtime_ms: Date.now(), local: false, tags: [] },
        { kind: "skill", name: "deploy", path: "C:/Users/me/.claude/skills/deploy/SKILL.md", size: 900, mtime_ms: Date.now(), local: false, description: "ship it", tags: ["ops"] },
        { kind: "agent", name: "reviewer", path: "C:/Users/me/.claude/agents/reviewer.md", size: 700, mtime_ms: Date.now(), local: false, tags: [] },
      ] }];
      case "harness_read": return "# stand-in\n\nfile text";
      // Per-server views: the remote box answers like a deck-server older than this Deck.
      case "archive_list": {
        if (String(args.server).includes("build-box")) throw new Error("the server refused deck.archive.scan: not a deck machine method");
        const now = Date.now();
        return [{ id: `${args.server}-a1b2c3d4-0000-0000-0000-000000000000`.slice(-36), source: "claude", label: "netrunner-deck", cwd: "C:/Workspace/netrunner-deck", linked_worktree: false,
          sessions: [{ tab_label: "", agent: "claude", session_id: "x", cwd: "C:/Workspace/netrunner-deck", resumable: true }],
          last_active_ms: now - 3600_000, prompt: `a session on ${args.server}`, prompts: 3, summary_state: "none", status: "closed", server: args.server }];
      }
      case "archive_groups_get": return [];
      case "uplink_list": case "uplink_seen": return { updates: [{ id: "u1", repo: "netrunner-deck", path: "C:/Workspace/netrunner-deck", branch: "main", from: "1db958d", to: "2ab34cd", at_ms: Date.now() - 600_000, commits: [], seen: true, server: "deck-server/try" }] };
      case "uplink_sync": case "uplink_fetch": return {};
      case "quickhacks_get": return { collapsed: false, hacks: [
        { id: "h1", name: "deploy", kind: "skill", source: "personal", path: "C:/Users/me/.claude/skills/deploy/SKILL.md", description: "ship it" },
        { id: "h2", name: "review", kind: "command", source: "personal", path: "/home/dev/.claude/commands/review.md", server: "dev@build-box.example.com/deck-server" },
      ] };
      case "quickhacks_set": return args.saved;
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
