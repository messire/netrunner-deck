import type { Agent, CswapAccount, Tree } from "./api";

/**
 * M3: global hotkeys and the command palette, as pure functions so they can be
 * tested without a DOM. Deck's hotkeys never reach the pane's PTY: F1, F2 and
 * Ctrl+K are claimed here; Esc is claimed only while an overlay is open, so
 * the CLI keeps Esc (interrupt) the rest of the time.
 */
export type Hotkey = "accounts" | "jump" | "palette" | "close";

export interface KeyLike { key: string; ctrlKey: boolean; altKey: boolean; metaKey: boolean; shiftKey: boolean }

/** Which Deck action a key press is, if any. `overlayOpen` lets Esc close it. */
export function hotkeyOf(e: KeyLike, overlayOpen: boolean): Hotkey | null {
  const plain = !e.ctrlKey && !e.altKey && !e.metaKey;
  if (e.key === "F1" && plain) return "accounts";
  if (e.key === "F2" && plain) return "jump";
  if ((e.key === "k" || e.key === "K") && e.ctrlKey && !e.altKey && !e.metaKey) return "palette";
  if (e.key === "Escape" && plain && overlayOpen) return "close";
  return null;
}

/**
 * F2: the next agent that waits for a human, after the current pane in server
 * order (wrapping). Null when nobody is blocked.
 */
export function nextBlockedPane(tree: Tree | undefined, currentPane: string | null): string | null {
  const blocked = (tree?.agents ?? []).filter((a) => a.agent_status === "blocked");
  if (!blocked.length) return null;
  const i = blocked.findIndex((a) => a.pane_id === currentPane);
  return blocked[(i + 1) % blocked.length].pane_id;
}

export interface Command {
  id: string;
  label: string;
  hint?: string;
  keys?: string;
  danger?: boolean;
  /** What the command does, as data: App turns it into an API call. */
  do:
    | { kind: "select"; paneId: string }
    | { kind: "switch"; account: number }
    | { kind: "action"; action: "workspace.create" | "workspace.close" | "tab.create" | "tab.close" | "pane.close" | "pane.split"; id?: string }
    | { kind: "refresh" };
}

/** Palette entries for the current tree and accounts; order: context actions, agents, accounts. */
export function commands(tree: Tree | undefined, accounts: CswapAccount[], currentPane: string | null): Command[] {
  const out: Command[] = [];
  const pane = tree?.panes.find((p) => p.pane_id === currentPane);
  const ws = pane && tree?.workspaces.find((w) => w.workspace_id === pane.workspace_id);
  const tab = pane && tree?.tabs.find((t) => t.tab_id === pane.tab_id);
  out.push({ id: "ws.new", label: "NEW SPACE", hint: "workspace.create", do: { kind: "action", action: "workspace.create", id: ws?.workspace_id } });
  if (ws) {
    out.push({ id: "tab.new", label: `NEW TAB IN ${ws.label.toUpperCase()}`, hint: "tab.create", do: { kind: "action", action: "tab.create", id: ws.workspace_id } });
    if (tab) out.push({ id: "tab.close", label: `CLOSE TAB ${tab.label.toUpperCase()}`, hint: "tab.close", danger: true, do: { kind: "action", action: "tab.close", id: tab.tab_id } });
    out.push({ id: "ws.close", label: `CLOSE SPACE ${ws.label.toUpperCase()}`, hint: "workspace.close", danger: true, do: { kind: "action", action: "workspace.close", id: ws.workspace_id } });
  }
  out.push({ id: "accounts.refresh", label: "REFRESH ACCOUNTS", hint: "cswap list", do: { kind: "refresh" } });
  for (const a of tree?.agents ?? []) {
    out.push({
      id: `agent:${a.pane_id}`,
      label: `${a.agent_status === "blocked" ? "◈" : a.agent_status === "working" ? "◆" : "◇"} ${agentName(tree, a)}`,
      hint: `${(a.display_agent ?? a.agent ?? "").toUpperCase()} · ${a.agent_status.toUpperCase()}`,
      keys: `${a.pane_id} ${a.workspace_id} agent open`,
      do: { kind: "select", paneId: a.pane_id },
    });
  }
  for (const acc of accounts) {
    out.push({
      id: `account:${acc.number}`,
      label: `${acc.active ? "◆" : "◇"} ACCOUNT ${acc.alias ?? acc.email}`,
      hint: acc.active ? "ACTIVE" : "cswap switch",
      keys: `${acc.email} account switch`,
      do: { kind: "switch", account: acc.number },
    });
  }
  return out;
}

/** Session name as the sidebar shows it: workspace (branch), plus the tab when the workspace hosts several agents. */
export function agentName(tree: Tree | undefined, a: Agent): string {
  if (a.name) return a.name;
  const ws = tree?.workspaces.find((w) => w.workspace_id === a.workspace_id);
  const wsName = ws?.worktree?.branch ?? ws?.label ?? a.workspace_id;
  const siblings = tree?.agents.filter((x) => x.workspace_id === a.workspace_id).length ?? 1;
  if (siblings <= 1) return wsName;
  const tabLabel = tree?.tabs.find((x) => x.tab_id === a.tab_id)?.label ?? a.tab_id;
  return `${wsName} · ${tabLabel}`;
}
