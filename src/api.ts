import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

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

export const api = {
  ping: () => invoke<{ version: string; protocol: number }>("herdr_ping"),
  tree: () => invoke<Tree>("herdr_tree"),
  paneRead: (paneId: string, lines = 400) =>
    invoke<{ read: { text: string; revision: number } }>("herdr_pane_read", { paneId, lines }),
  sendText: (paneId: string, text: string) => invoke("herdr_pane_send_text", { paneId, text }),
  sendKeys: (paneId: string, keys: string[]) => invoke("herdr_pane_send_keys", { paneId, keys }),
  focus: (target: "workspace" | "tab" | "pane", id: string) => invoke("herdr_focus", { target, id }),
  cswapList: () => invoke<CswapList>("cswap_list"),
  cswapSwitch: (number: number) => invoke("cswap_switch", { number }),
  onEvent: (cb: (event: Record<string, unknown>) => void): Promise<UnlistenFn> =>
    listen<Record<string, unknown>>("herdr://event", (e) => cb(e.payload)),
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
