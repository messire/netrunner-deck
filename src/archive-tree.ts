import type { ArchiveEntry } from "./api";
import { matches } from "./archive";

/**
 * The ARCHIVE tree: repository › worktree › session, like the Sessions tree
 * in the sidebar. Where an entry belongs comes from Herdr's worktree facts
 * when it has them (source HERDR) and from the cwd otherwise:
 *   …/.herdr/worktrees/<repo>/<wt>/…  → repo <repo>, worktree <wt>
 *   …/<repo>/.claude/worktrees/<wt>/… → repo <repo>, worktree <wt>
 *   anything else                     → repo = that folder, worktree "root"
 * Pure functions; the component only renders what comes out.
 */
export interface SessionNode { kind: "session"; entry: ArchiveEntry }
export interface WorktreeNode {
  kind: "worktree";
  id: string; // `${repoKey}|${pathKey}`
  name: string; // branch / worktree folder, "root" for the main checkout
  path: string;
  root: boolean; // the repository's own checkout: never removable
  linked: boolean; // a linked git worktree that DELETE may remove
  sessions: SessionNode[];
  lastActive: number;
}
export interface RepoNode {
  kind: "repo";
  id: string; // repo key (lowercased name)
  name: string;
  worktrees: WorktreeNode[];
  sessions: number;
  lastActive: number;
}

const norm = (p: string) => p.replace(/^\\\\\?\\/, "").replace(/[\\/]+/g, "/").replace(/\/$/, "");
const base = (p: string) => norm(p).split("/").filter(Boolean).pop() ?? p;

export interface Placement { repo: string; repoKey: string; worktree: string; path: string; root: boolean; linked: boolean }

/** Where one entry sits in the tree. */
export function placement(e: ArchiveEntry): Placement {
  const cwd = norm(e.cwd);
  if (e.repo_root) {
    const repo = e.repo_name || base(e.repo_root);
    if (e.linked_worktree) {
      return { repo, repoKey: repo.toLowerCase(), worktree: e.branch || base(cwd), path: cwd, root: false, linked: true };
    }
    return { repo, repoKey: repo.toLowerCase(), worktree: "root", path: norm(e.repo_root), root: true, linked: false };
  }
  let m = /^(.*)\/\.herdr\/worktrees\/([^/]+)\/([^/]+)(\/.*)?$/i.exec(cwd);
  if (m) return { repo: m[2], repoKey: m[2].toLowerCase(), worktree: m[3], path: `${m[1]}/.herdr/worktrees/${m[2]}/${m[3]}`, root: false, linked: true };
  m = /^(.*\/([^/]+))\/\.claude\/worktrees\/([^/]+)(\/.*)?$/i.exec(cwd);
  if (m) return { repo: m[2], repoKey: m[2].toLowerCase(), worktree: m[3], path: `${m[1]}/.claude/worktrees/${m[3]}`, root: false, linked: true };
  const repo = base(cwd) || cwd;
  return { repo, repoKey: repo.toLowerCase(), worktree: "root", path: cwd, root: true, linked: false };
}

/** Group entries (already filtered) into repos › worktrees › sessions, newest activity first at every level. */
export function buildArchiveTree(entries: ArchiveEntry[], query = ""): RepoNode[] {
  const repos = new Map<string, RepoNode>();
  for (const e of entries) {
    if (!matches(e, query)) continue;
    const p = placement(e);
    let r = repos.get(p.repoKey);
    if (!r) { r = { kind: "repo", id: p.repoKey, name: p.repo, worktrees: [], sessions: 0, lastActive: 0 }; repos.set(p.repoKey, r); }
    const wid = `${p.repoKey}|${p.path.toLowerCase()}`;
    let w = r.worktrees.find((x) => x.id === wid);
    if (!w) { w = { kind: "worktree", id: wid, name: p.worktree, path: p.path, root: p.root, linked: p.linked, sessions: [], lastActive: 0 }; r.worktrees.push(w); }
    w.sessions.push({ kind: "session", entry: e });
    w.lastActive = Math.max(w.lastActive, e.last_active_ms);
    r.lastActive = Math.max(r.lastActive, e.last_active_ms);
    r.sessions += 1;
  }
  const byActivity = (a: { lastActive: number }, b: { lastActive: number }) => b.lastActive - a.lastActive;
  const out = [...repos.values()].sort(byActivity);
  for (const r of out) {
    r.worktrees.sort((a, b) => (a.root === b.root ? byActivity(a, b) : a.root ? -1 : 1));
    for (const w of r.worktrees) w.sessions.sort((a, b) => (a.entry.status === b.entry.status ? b.entry.last_active_ms - a.entry.last_active_ms : a.entry.status === "closed" ? -1 : 1));
  }
  return out;
}

/**
 * The delete plan for a node: every session id under it; the last entry of a
 * linked worktree carries the `git worktree remove`. Root checkouts are never
 * removed, only their sessions.
 */
export function deletePlan(node: RepoNode | WorktreeNode): { id: string; removeWorktree: boolean }[] {
  const worktrees = node.kind === "repo" ? node.worktrees : [node];
  const out: { id: string; removeWorktree: boolean }[] = [];
  for (const w of worktrees) {
    w.sessions.forEach((s, i) => out.push({ id: s.entry.id, removeWorktree: w.linked && !w.root && i === w.sessions.length - 1 }));
  }
  return out;
}
