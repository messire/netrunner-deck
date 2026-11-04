import type { UnlistenFn } from "@tauri-apps/api/event";
import { invoke, listen } from "./tauri";

export type AgentStatus = "idle" | "working" | "blocked" | "done" | "unknown";

export interface Workspace {
  workspace_id: string;
  number: number;
  label: string;
  focused: boolean;
  pane_count: number;
  tab_count: number;
  active_tab_id: string;
  agent_status: AgentStatus;
  tokens?: Record<string, string>;
  worktree?: { repo_key: string; repo_name: string; branch?: string } | null;
}
export interface Tab {
  tab_id: string;
  workspace_id: string;
  number: number;
  label: string;
  focused: boolean;
  pane_count: number;
  agent_status: AgentStatus;
}
export interface Pane {
  pane_id: string;
  terminal_id: string;
  workspace_id: string;
  tab_id: string;
  focused: boolean;
  cwd?: string | null;
  label?: string | null;
  agent?: string | null;
  display_agent?: string | null;
  title?: string | null;
  agent_status: AgentStatus;
  tokens?: Record<string, string>;
}
export interface Agent {
  terminal_id: string;
  name?: string | null;
  agent?: string | null;
  display_agent?: string | null;
  agent_status: AgentStatus;
  tokens?: Record<string, string>;
  workspace_id: string;
  tab_id: string;
  pane_id: string;
  focused: boolean;
}
export interface Tree { workspaces: Workspace[]; tabs: Tab[]; panes: Pane[]; agents: Agent[] }

export interface CswapAccount {
  number: number;
  email: string;
  alias?: string;
  active: boolean;
  usageStatus?: string;
  usage?: CswapUsage;
  lastGoodUsage?: CswapUsage;
}
export interface CswapUsage {
  fiveHour?: { pct: number; countdown?: string };
  sevenDay?: { pct: number; countdown?: string };
  scoped?: { name: string; pct: number; countdown?: string }[];
}
export interface CswapList { activeAccountNumber: number; accounts: CswapAccount[] }

/**
 * One event from Herdr's `events.subscribe` stream, forwarded as-is by the
 * Rust pump on `herdr://event`. `event` is the variant name in snake_case
 * (`pane_updated`, `workspace_created`, …) and the remaining fields are that
 * variant's payload — e.g. `pane_updated` carries `pane: PaneInfo` and is
 * metadata, not output. See docs/architecture.md F1–F3. Routing: `src/events.ts`.
 */
export type HerdrEvent = { event: string; [k: string]: unknown };

export type TreeAction = "workspace.create" | "workspace.close" | "tab.create" | "tab.close" | "pane.close";

/**
 * `pane.read` reply. `revision` is the fork's content counter (0 upstream).
 * `cursor` and `alternate_screen` are fork additions (M2): `cursor.line` is
 * the cursor's row as an index into the lines of `text` (absent when the
 * server cannot tell), so the view places a real cursor instead of guessing.
 */
export interface PaneRead {
  text: string;
  revision: number;
  cursor?: PaneCursor | null;
  alternate_screen?: boolean;
}
export interface PaneCursor {
  line?: number | null;
  col: number;
  visible: boolean;
  /** DECSCUSR 0–6; 0 = terminal default. */
  shape: number;
}

// DEV-only call counters for manual checks: Tauri `invoke` is not visible in the
// DevTools Network tab, so inspect `window.__deckCalls` in the webview console.
declare global { interface Window { __deckCalls?: Record<string, number> } }
export const callCounts: Record<string, number> = {};
const count = (name: string) => {
  if (!import.meta.env.DEV) return;
  callCounts[name] = (callCounts[name] ?? 0) + 1;
  if (typeof window !== "undefined") window.__deckCalls = callCounts;
};

/** A newer signed release the updater found (M5). */
export type UpdateInfo = { version: string; current: string; notes: string | null; date: string | null };

export const api = {
  ping: () => invoke<{ version: string; protocol: number }>("herdr_ping"),
  tree: () => { count("herdr_tree"); return invoke<Tree>("herdr_tree"); },
  paneRead: (paneId: string, lines = 400) => {
    count("herdr_pane_read");
    return invoke<{ read: PaneRead }>("herdr_pane_read", { paneId, lines });
  },
  sendText: (paneId: string, text: string) => invoke("herdr_pane_send_text", { paneId, text }),
  sendKeys: (paneId: string, keys: string[]) => invoke("herdr_pane_send_keys", { paneId, keys }),
  focus: (target: "workspace" | "tab" | "pane", id: string) => invoke("herdr_focus", { target, id }),
  /** Tree actions (M3); the Rust side maps the name to a Herdr method with a fixed param set. */
  action: (action: TreeAction, id?: string | null, options?: { cwd?: string; label?: string; close_group?: boolean }) =>
    invoke("herdr_action", { action, id: id ?? null, options: options ?? null }),
  /** OS notification (M4); used when an agent starts waiting while its pane is not on screen. */
  notify: (title: string, body: string) => invoke<void>("notify", { title, body }),
  /** Updater (M5): `null` when up to date; errors (offline, no release yet) are advisory. */
  updateCheck: () => invoke<UpdateInfo | null>("update_check"),
  updateInstall: () => invoke<void>("update_install"),
  cswapList: () => { count("cswap_list"); return invoke<CswapList>("cswap_list"); },
  cswapSwitch: (number: number) => { count("cswap_switch"); return invoke("cswap_switch", { number }); },
  // The payload is whatever Herdr sent; `routeEvent` validates the shape.
  onEvent: (cb: (event: HerdrEvent) => void): Promise<UnlistenFn> =>
    listen<HerdrEvent>("herdr://event", (e) => cb(e.payload)),
  onOffline: (cb: (message: string) => void): Promise<UnlistenFn> =>
    listen<string>("herdr://offline", (e) => cb(e.payload)),
};

export function accountLabel(a: CswapAccount): string {
  const alias = (a.alias ?? "").trim();
  if (alias) return alias.toUpperCase();
  const [user, domain = ""] = a.email.split("@");
  const org = domain.split(".")[0];
  return (/gmail|proton|outlook|hotmail|yahoo/i.test(org) ? user : org).toUpperCase();
}

export function usageOf(a: CswapAccount) {
  const u = a.usage ?? a.lastGoodUsage ?? {};
  const pct = (x?: number) => (typeof x === "number" && isFinite(x) ? Math.max(0, Math.min(100, Math.round(x))) : null);
  return {
    five: pct(u.fiveHour?.pct), fiveReset: u.fiveHour?.countdown,
    week: pct(u.sevenDay?.pct), weekReset: u.sevenDay?.countdown,
    scoped: (u.scoped ?? []).map((s) => ({ name: s.name, pct: pct(s.pct), reset: s.countdown })),
  };
}

export type Zone = "safe" | "public" | "restricted" | "danger";
export function zoneOf(a?: CswapAccount): Zone {
  if (!a) return "danger";
  if (a.usageStatus && /relogin|expired|error/i.test(a.usageStatus)) return "danger";
  const { five, week } = usageOf(a);
  const worst = Math.max(five ?? 0, week ?? 0);
  if (worst >= 100) return "danger";
  if (worst >= 80) return "restricted";
  if (worst >= 50) return "public";
  return "safe";
}
