import type { Agent, AgentStatus, Pane, Tab, Tree, Workspace } from "./api";

/**
 * Sidebar tree model: repo › space (workspace) › tab › session (pane).
 *
 * Herdr's own aggregates (`agent_status` on tab/workspace) are kept as the
 * truth for those levels; only the repo level is rolled up here. A tab with a
 * single pane collapses into one session row; a split tab keeps the tab row
 * and lists its sessions one level deeper. Pure functions, covered by
 * `tree.test.ts`.
 */

export interface SessionNode {
  kind: "session";
  id: string;
  pane: Pane;
  agent: Agent | null;
  status: AgentStatus;
  /** Agent kind (`claude`, `codex`) or null for a plain shell. */
  agentKind: string | null;
  label: string;
}
export interface TabNode {
  kind: "tab";
  id: string;
  tab: Tab;
  status: AgentStatus;
  sessions: SessionNode[];
  /** Set when the tab holds exactly one pane: the row is that session. */
  single: SessionNode | null;
}
export interface SpaceNode {
  kind: "space";
  id: string;
  workspace: Workspace;
  status: AgentStatus;
  /** Primary checkout of its repo (`worktree` present, not a linked worktree). */
  root: boolean;
  linked: boolean;
  tabs: TabNode[];
  agents: number;
}
export interface RepoNode {
  kind: "repo";
  id: string;
  label: string;
  status: AgentStatus;
  spaces: SpaceNode[];
  agents: number;
}
export type RootNode = RepoNode | SpaceNode;

/** Lower = more urgent. `unknown` (shell, no agent) sorts last. */
export const RANK: Record<AgentStatus, number> = { blocked: 0, working: 1, idle: 2, done: 3, unknown: 4 };

export const rollup = (statuses: AgentStatus[]): AgentStatus =>
  statuses.reduce<AgentStatus>((acc, s) => ((RANK[s] ?? 9) < (RANK[acc] ?? 9) ? s : acc), "unknown");

export const sessionLabel = (pane: Pane, agent: Agent | null): string =>
  pane.label || agent?.name || agent?.display_agent || agent?.agent || pane.display_agent || pane.agent || pane.title || "shell";

export function buildTree(t: Tree | undefined): RootNode[] {
  if (!t) return [];
  const agentByPane = new Map(t.agents.map((a) => [a.pane_id, a] as const));
  const session = (pane: Pane): SessionNode => {
    const agent = agentByPane.get(pane.pane_id) ?? null;
    return {
      kind: "session",
      id: pane.pane_id,
      pane,
      agent,
      status: agent?.agent_status ?? (pane.agent ? pane.agent_status : "unknown"),
      agentKind: agent?.display_agent ?? agent?.agent ?? pane.display_agent ?? pane.agent ?? null,
      label: sessionLabel(pane, agent),
    };
  };
  const tabNode = (tab: Tab): TabNode => {
    const sessions = t.panes.filter((p) => p.tab_id === tab.tab_id).map(session);
    return { kind: "tab", id: tab.tab_id, tab, status: tab.agent_status, sessions, single: sessions.length === 1 ? sessions[0] : null };
  };
  const spaceNode = (w: Workspace): SpaceNode => {
    const tabs = t.tabs.filter((x) => x.workspace_id === w.workspace_id).map(tabNode);
    const agents = tabs.reduce((n, tb) => n + tb.sessions.filter((s) => s.agent).length, 0);
    const linked = w.worktree?.is_linked_worktree === true;
    return { kind: "space", id: w.workspace_id, workspace: w, status: w.agent_status, root: !!w.worktree && !linked, linked, tabs, agents };
  };

  const roots: RootNode[] = [];
  const repos = new Map<string, RepoNode>();
  for (const w of t.workspaces) {
    const node = spaceNode(w);
    if (!w.worktree) { roots.push(node); continue; }
    const key = w.worktree.repo_key;
    let repo = repos.get(key);
    if (!repo) {
      repo = { kind: "repo", id: `repo:${key}`, label: w.worktree.repo_name, status: "unknown", spaces: [], agents: 0 };
      repos.set(key, repo);
      roots.push(repo);
    }
    repo.spaces.push(node);
  }
  for (const r of repos.values()) {
    r.agents = r.spaces.reduce((n, s) => n + s.agents, 0);
    r.status = rollup(r.spaces.map((s) => s.status));
  }
  return roots;
}

export const spaces = (roots: RootNode[]): SpaceNode[] => roots.flatMap((r) => (r.kind === "repo" ? r.spaces : [r]));

/** Does this node (at any depth) contain the pane? */
export function contains(node: RootNode | TabNode, paneId: string | null): boolean {
  if (!paneId) return false;
  if (node.kind === "tab") return node.sessions.some((s) => s.id === paneId);
  if (node.kind === "space") return node.tabs.some((tb) => contains(tb, paneId));
  return node.spaces.some((s) => contains(s, paneId));
}

/** Open by default when something in it needs eyes or is on screen. */
export const defaultOpen = (node: RootNode | TabNode, selectedPane: string | null): boolean =>
  node.status === "blocked" || node.status === "working" || contains(node, selectedPane);

/** First pane to show for a node: the focused one, else the first. */
export function firstPane(node: RootNode | TabNode): Pane | null {
  const panes: Pane[] =
    node.kind === "tab" ? node.sessions.map((s) => s.pane)
    : node.kind === "space" ? node.tabs.flatMap((tb) => tb.sessions.map((s) => s.pane))
    : node.spaces.flatMap((s) => s.tabs.flatMap((tb) => tb.sessions.map((x) => x.pane)));
  return panes.find((p) => p.focused) ?? panes[0] ?? null;
}

export interface Waiting { session: SessionNode; path: string }

/** Blocked sessions with their `repo / space / tab` path, in tree order. */
export function waiting(roots: RootNode[]): Waiting[] {
  const out: Waiting[] = [];
  const walk = (space: SpaceNode, repo?: RepoNode) => {
    for (const tb of space.tabs) {
      for (const s of tb.sessions) {
        if (s.status !== "blocked") continue;
        // Repeated names collapse: a root checkout named like its repo, a tab named like its space.
        const parts = [repo?.label];
        if (space.workspace.label !== repo?.label) parts.push(space.workspace.label);
        if (!tb.single || tb.tab.label !== space.workspace.label) parts.push(tb.tab.label);
        if (!tb.single) parts.push(s.label);
        out.push({ session: s, path: parts.filter(Boolean).join(" / ") });
      }
    }
  };
  for (const r of roots) r.kind === "repo" ? r.spaces.forEach((s) => walk(s, r)) : walk(r);
  return out;
}

export const ctxPct = (tokens?: Record<string, string>): number | null => {
  const n = parseInt(tokens?.ctx ?? "", 10);
  return Number.isFinite(n) ? n : null;
};

/** Per-node open/closed overrides, kept in localStorage (`deck.tree.open`). */
export const OPEN_KEY = "deck.tree.open";
export function loadOpen(storage: Pick<Storage, "getItem"> | undefined): Record<string, boolean> {
  try {
    const raw = storage?.getItem(OPEN_KEY);
    const v = raw ? JSON.parse(raw) : {};
    return v && typeof v === "object" ? v : {};
  } catch { return {}; }
}
export function saveOpen(storage: Pick<Storage, "setItem"> | undefined, open: Record<string, boolean>): void {
  try { storage?.setItem(OPEN_KEY, JSON.stringify(open)); } catch { /* private mode etc. */ }
}
