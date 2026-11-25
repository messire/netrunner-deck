import type { AgentStatus, HarnessItem } from "./api";
import type { RepoNode, SpaceNode } from "./tree";

/**
 * COPY TO / MOVE TO another server, and clone addresses — the parts without
 * DOM. The sidebar's own model (`tree.ts`: repo › space › tab › session) is
 * what a transfer is chosen from: a repository, a branch (a space: its
 * worktree) or a session (a pane with a Claude session).
 */

export interface TSession { paneId: string; sessionId: string | null; cwd: string; label: string; status: AgentStatus }
export interface TBranch { workspaceId: string; path: string; branch: string; root: boolean; sessions: TSession[] }
export interface TRepo { label: string; root: string; branches: TBranch[] }

/** The repository a sidebar repo node stands for; `sessionOf` gives a pane's Claude session id. */
export function repoOf(node: RepoNode, sessionOf: (paneId: string) => string | null): TRepo {
  const branches = node.spaces.map((s) => branchOf(s, sessionOf)).filter((b): b is TBranch => !!b);
  const root = branches.find((b) => b.root)?.path ?? branches[0]?.path ?? "";
  return { label: node.label, root, branches };
}

export function branchOf(space: SpaceNode, sessionOf: (paneId: string) => string | null): TBranch | null {
  const wt = space.workspace.worktree;
  if (!wt?.checkout_path) return null;
  // A tab with one pane is shown as that session under the tab's name.
  const sessions = space.tabs.flatMap((t) => t.sessions.map((s) => ({
    paneId: s.id, sessionId: sessionOf(s.id), cwd: s.pane.cwd ?? wt.checkout_path!, label: t.single ? t.tab.label : s.label, status: s.status,
  })));
  return { workspaceId: space.id, path: wt.checkout_path, branch: wt.branch ?? space.workspace.label, root: !wt.is_linked_worktree, sessions };
}

/** What is chosen: everything, or branches (by workspace id) and sessions (by pane id). */
export interface Choice { whole: boolean; branches: Set<string>; sessions: Set<string> }

export const everything = (): Choice => ({ whole: true, branches: new Set(), sessions: new Set() });

/** A choice for one branch: all its sessions, or only `session`. */
export function choiceForBranch(b: TBranch, session?: string): Choice {
  return { whole: false, branches: new Set([b.workspaceId]), sessions: new Set(session ? [session] : b.sessions.map((s) => s.paneId)) };
}

export interface Plan {
  whole: boolean;
  repo: string;
  name: string;
  worktrees: { path: string; branch: string; root: boolean; workspaceId: string }[];
  sessions: { session_id: string; cwd: string; pane_id: string; label: string; status: AgentStatus }[];
}

/** The folder name of a path (`C:\x\atlas` → `atlas`). */
export const baseName = (path: string) => path.replace(/[\\/]+$/, "").split(/[\\/]/).pop() ?? path;

/** What a choice transfers; a session's branch always comes with it. */
export function planOf(repo: TRepo, c: Choice): Plan {
  const picked = (b: TBranch) => c.whole || c.branches.has(b.workspaceId) || b.sessions.some((s) => c.sessions.has(s.paneId));
  const branches = repo.branches.filter(picked);
  const sessions = branches.flatMap((b) => b.sessions.filter((s) => s.sessionId && (c.whole || c.sessions.has(s.paneId))));
  return {
    whole: c.whole,
    repo: repo.root,
    name: baseName(repo.root),
    worktrees: branches.map((b) => ({ path: b.path, branch: b.branch, root: b.root, workspaceId: b.workspaceId })),
    sessions: sessions.map((s) => ({ session_id: s.sessionId!, cwd: s.cwd, pane_id: s.paneId, label: s.label, status: s.status })),
  };
}

/** A chosen session that is working or waiting for an answer: it cannot be packed now. */
export function busyOf(plan: Plan): string | null {
  const busy = plan.sessions.filter((s) => s.status === "working" || s.status === "blocked");
  if (!busy.length) return null;
  const names = busy.map((s) => s.label).join(", ");
  return `${names} ${busy.length > 1 ? "are" : "is"} busy — wait until ${busy.length > 1 ? "they are" : "it is"} idle (or stop ${busy.length > 1 ? "them" : "it"}) and try again`;
}

const plural = (n: number, one: string) => `${n} ${one}${n === 1 ? "" : "s"}`;

/** The one line a MOVE states before it starts. */
export function removalText(plan: Plan, source: string, target: string): string {
  const after = `from ${source} after it arrives on ${target}`;
  if (plan.whole) return `This removes the repository ${plan.name}, all its worktrees and sessions ${after}.`;
  const linked = plan.worktrees.filter((w) => !w.root);
  const parts: string[] = [];
  if (linked.length) parts.push(linked.length === 1 ? `the worktree ${linked[0].branch} and its branch` : `${plural(linked.length, "worktree")} and their branches`);
  if (plan.sessions.length) parts.push(plural(plan.sessions.length, "session"));
  if (!parts.length) return `Nothing is removed from ${source}: the main checkout is the repository itself and stays (move the whole repository, or a linked worktree).`;
  const what = parts.join(" and ");
  const keeps = plan.worktrees.some((w) => w.root) ? " (the main checkout stays)" : "";
  return `This removes ${what} ${after}${keeps}.`;
}

/** `deck.transfer.export.start` parameters. */
export const exportParams = (p: Plan) => ({
  kind: "repo", repo: p.repo, whole: p.whole,
  worktrees: p.worktrees.map((w) => ({ path: w.path, branch: w.branch })),
  sessions: p.sessions.map((s) => ({ session_id: s.session_id, cwd: s.cwd })),
});

/** `deck.transfer.remove.start` parameters (after a MOVE arrived). */
export const removeParams = (p: Plan) => ({
  kind: "repo", repo: p.repo, whole: p.whole,
  worktrees: p.worktrees.map((w) => ({ path: w.path })),
  sessions: p.sessions.map((s) => s.session_id),
});

// ---- clone addresses ---------------------------------------------------------------

/** A git address to clone: https/http/ssh/git URL or scp-like `user@host:path`. */
export function isGitUrl(text: string): boolean {
  const s = text.trim();
  if (!s || /\s/.test(s)) return false;
  const m = /^(https?|ssh|git|git\+ssh):\/\/([^/]+)\/(.+)$/.exec(s);
  if (m) return !!m[2] && !!m[3].replace(/\/+$/, "");
  const scp = /^([^@/\\:]+)@([^/\\:]+):(.+)$/.exec(s);
  return !!scp && !!scp[3].replace(/^\/+|\/+$/g, "");
}

/** The folder a clone gets: the last path part without `.git`. */
export function repoNameOf(url: string): string {
  const last = url.trim().replace(/\/+$/, "").split(/[/:]/).pop() ?? "";
  return last.replace(/\.git$/, "") || "repo";
}

/** A github.com address (any form): GitHub sign-in can help when it is refused. */
export const isGitHub = (url: string) => /^(https?:\/\/([^@/]+@)?(www\.)?github\.com\/|ssh:\/\/([^@/]+@)?github\.com[/:]|[^@/]+@github\.com:)/i.test(url.trim());

/** One clone status as the dialog line says it. */
export function cloneLine(s: { phase?: string; percent?: number | null }): string {
  const phase = (s.phase ?? "").trim() || "Cloning";
  return typeof s.percent === "number" ? `${phase} · ${s.percent}%` : `${phase}…`;
}

// ---- personal harness ------------------------------------------------------------------

/** Kinds that may travel between machines (settings, MCP and hooks stay). */
const TRAVELS = new Set(["claude_md", "skill", "agent", "command", "rule"]);

/** The item as a path under `~/.claude` (`skills/x` for a skill's folder), or null when it stays. */
export function harnessRel(item: HarnessItem, personalRoot: string): string | null {
  if (!TRAVELS.has(item.kind)) return null;
  const norm = (p: string) => p.replace(/^\\\\\?\\/, "").replace(/[\\/]+/g, "/").replace(/\/$/, "");
  const root = norm(personalRoot);
  let path = norm(item.path);
  if (!path.toLowerCase().startsWith(root.toLowerCase() + "/")) return null;
  let rel = path.slice(root.length + 1);
  if (item.kind === "skill") rel = rel.replace(/\/SKILL\.md$/i, "");
  if (item.kind === "claude_md" && rel !== "CLAUDE.md") return null;
  path = rel;
  return path;
}
