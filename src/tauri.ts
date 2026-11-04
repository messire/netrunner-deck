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
    { workspace_id: "w1", number: 1, label: "Elly2", focused: true, pane_count: 2, tab_count: 2, active_tab_id: "w1:t1", agent_status: "working" },
    { workspace_id: "w2", number: 2, label: "SRE", focused: false, pane_count: 1, tab_count: 1, active_tab_id: "w2:t1", agent_status: "blocked" },
    { workspace_id: "w3", number: 3, label: "attr-perf", focused: false, pane_count: 1, tab_count: 1, active_tab_id: "w3:t1", agent_status: "idle", worktree: { repo_key: "elly", repo_name: "Elly2", branch: "attr-perf" } },
  ];
  const tabs = [
    { tab_id: "w1:t1", workspace_id: "w1", number: 1, label: "main", focused: true, pane_count: 1, agent_status: "working" },
    { tab_id: "w1:t2", workspace_id: "w1", number: 2, label: "tests", focused: false, pane_count: 1, agent_status: "blocked" },
    { tab_id: "w2:t1", workspace_id: "w2", number: 1, label: "ops", focused: true, pane_count: 1, agent_status: "blocked" },
    { tab_id: "w3:t1", workspace_id: "w3", number: 1, label: "1", focused: true, pane_count: 1, agent_status: "idle" },
  ];
  const panes = [
    { pane_id: "w1:p1", terminal_id: "t1", workspace_id: "w1", tab_id: "w1:t1", focused: true, cwd: "C:\\Workspace\\Elly2", agent: "claude", display_agent: "Claude", title: "claude", agent_status: "working", tokens: { ctx: "42" } },
    { pane_id: "w1:p2", terminal_id: "t2", workspace_id: "w1", tab_id: "w1:t2", focused: false, cwd: "C:\\Workspace\\Elly2", agent: "claude", agent_status: "blocked", tokens: { ctx: "71" } },
    { pane_id: "w2:p1", terminal_id: "t3", workspace_id: "w2", tab_id: "w2:t1", focused: true, cwd: "C:\\Workspace\\SRE", agent: "claude", agent_status: "blocked" },
    { pane_id: "w3:p1", terminal_id: "t4", workspace_id: "w3", tab_id: "w3:t1", focused: true, cwd: "C:\\Workspace\\Elly2-attr", agent: null, title: "pwsh", agent_status: "unknown" },
  ];
  const agents = () => panes.filter((p) => p.agent).map((p) => ({ terminal_id: p.terminal_id, agent: p.agent, display_agent: p.display_agent, agent_status: p.agent_status, tokens: p.tokens, workspace_id: p.workspace_id, tab_id: p.tab_id, pane_id: p.pane_id, focused: p.focused }));
  const accounts = {
    activeAccountNumber: 1,
    accounts: [
      { number: 1, email: "one@example.com", alias: "PLURIO", active: true, usage: { fiveHour: { pct: 7, countdown: "14m" }, sevenDay: { pct: 4, countdown: "6d 5h" } } },
      { number: 2, email: "two@example.com", alias: "PROTON", active: false, usage: { fiveHour: { pct: 4 }, sevenDay: { pct: 24 } } },
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
    if (grown < extra.length) { grown++; revision += 2; emit("herdr://event", { event: "pane_output_changed", data: { type: "pane_output_changed", pane_id: "w1:p1", workspace_id: "w1", revision } }); }
  }, 3000);

  const tree = () => ({ workspaces, tabs, panes, agents: agents() });
  const invoke = async (cmd: string, args: Record<string, unknown>): Promise<unknown> => {
    await new Promise((r) => setTimeout(r, 30));
    switch (cmd) {
      case "herdr_ping": return { version: "browser-fake", protocol: 0 };
      case "herdr_tree": return tree();
      case "herdr_pane_read": {
        const paneId = String(args.paneId);
        if (paneId === "w1:p1") return { read: transcript() };
        return { read: { text: `PS ${panes.find((p) => p.pane_id === paneId)?.cwd ?? "C:\\"}> echo ${paneId}\n${paneId}\nPS> `, revision: 0 } };
      }
      case "herdr_pane_send_text": case "herdr_pane_send_keys": case "herdr_focus": return null;
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
        } else if (action === "workspace.close" && target) {
          const w = workspaces.find((x) => x.workspace_id === target);
          if (w && !w.worktree && workspaces.some((x) => x.worktree?.repo_name === w.label) && !(args.options as any)?.close_group) throw new Error("herdr returned an error for workspace.close: workspace_group_close_required");
          for (let i = panes.length - 1; i >= 0; i--) if (panes[i].workspace_id === target) panes.splice(i, 1);
          for (let i = tabs.length - 1; i >= 0; i--) if (tabs[i].workspace_id === target) tabs.splice(i, 1);
          const i = workspaces.findIndex((x) => x.workspace_id === target); if (i >= 0) workspaces.splice(i, 1);
        }
        setTimeout(() => emit("herdr://event", { event: "layout_updated" }), 10);
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
