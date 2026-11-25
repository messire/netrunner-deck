import { describe, expect, it } from "vitest";
import type { Agent, Pane, Tab, Tree, Workspace } from "./api";
import { buildTree, type RepoNode } from "./tree";
import {
  busyOf, choiceForBranch, cloneLine, everything, exportParams, isGitHub, isGitUrl, planOf, removalText, removeParams, repoNameOf, repoOf,
} from "./transfer";

const ws = (id: string, label: string, extra: Partial<Workspace> = {}): Workspace => ({
  workspace_id: id, number: 1, label, focused: false, pane_count: 1, tab_count: 1, active_tab_id: `${id}:t1`, agent_status: "idle", ...extra,
});
const tab = (id: string, workspace_id: string, label: string): Tab => ({ tab_id: id, workspace_id, number: 1, label, focused: false, pane_count: 1, agent_status: "idle" });
const pane = (id: string, workspace_id: string, tab_id: string, extra: Partial<Pane> = {}): Pane => ({
  pane_id: id, terminal_id: `term-${id}`, workspace_id, tab_id, focused: false, agent_status: "idle", agent: "claude", ...extra,
});
const agent = (p: Pane, status: Agent["agent_status"] = "idle"): Agent => ({
  terminal_id: p.terminal_id, agent: "claude", agent_status: status, workspace_id: p.workspace_id, tab_id: p.tab_id, pane_id: p.pane_id, focused: false,
});
const atlas = { repo_key: "atlas", repo_name: "atlas" };

function fixture(featStatus: Agent["agent_status"] = "idle"): Tree {
  const p1 = pane("w1:p1", "w1", "w1:t1", { cwd: "C:\\w\\atlas" });
  const p2 = pane("w1:p2", "w1", "w1:t2", { cwd: "C:\\w\\atlas\\src" });
  const p3 = pane("w2:p1", "w2", "w2:t1", { cwd: "C:\\wt\\atlas\\feat" });
  return {
    workspaces: [
      ws("w1", "atlas", { worktree: { ...atlas, branch: "main", is_linked_worktree: false, checkout_path: "C:\\w\\atlas" }, tab_count: 2 }),
      ws("w2", "feat", { worktree: { ...atlas, branch: "feat/x", is_linked_worktree: true, checkout_path: "C:\\wt\\atlas\\feat" } }),
    ],
    tabs: [tab("w1:t1", "w1", "chat"), tab("w1:t2", "w1", "review"), tab("w2:t1", "w2", "feat chat")],
    panes: [p1, p2, p3],
    agents: [agent(p1), agent(p2), agent(p3, featStatus)],
  };
}
const sessions: Record<string, string> = { "w1:p1": "s-main", "w2:p1": "s-feat" };
const repo = (t = fixture()) => repoOf(buildTree(t)[0] as RepoNode, (id) => sessions[id] ?? null);

describe("transfer plans from the sidebar's tree", () => {
  it("a repository lists its branches with their sessions", () => {
    const r = repo();
    expect(r.root).toBe("C:\\w\\atlas");
    expect(r.branches.map((b) => [b.branch, b.root, b.sessions.map((s) => s.sessionId)])).toEqual([
      ["main", true, ["s-main", null]],
      ["feat/x", false, ["s-feat"]],
    ]);
  });

  it("everything takes every branch and every session that has a conversation", () => {
    const p = planOf(repo(), everything());
    expect(p.whole).toBe(true);
    expect(p.name).toBe("atlas");
    expect(p.worktrees.map((w) => w.branch)).toEqual(["main", "feat/x"]);
    expect(p.sessions.map((s) => s.session_id)).toEqual(["s-main", "s-feat"]);
    expect(exportParams(p)).toEqual({
      kind: "repo", repo: "C:\\w\\atlas", whole: true,
      worktrees: [{ path: "C:\\w\\atlas", branch: "main" }, { path: "C:\\wt\\atlas\\feat", branch: "feat/x" }],
      sessions: [{ session_id: "s-main", cwd: "C:\\w\\atlas" }, { session_id: "s-feat", cwd: "C:\\wt\\atlas\\feat" }],
    });
  });

  it("a session brings its branch; a branch alone brings no sessions", () => {
    const r = repo();
    const p = planOf(r, { whole: false, branches: new Set(), sessions: new Set(["w2:p1"]) });
    expect(p.worktrees.map((w) => w.branch)).toEqual(["feat/x"]);
    expect(p.sessions.map((s) => s.session_id)).toEqual(["s-feat"]);
    const only = planOf(r, { whole: false, branches: new Set(["w1"]), sessions: new Set() });
    expect(only.worktrees.map((w) => w.branch)).toEqual(["main"]);
    expect(only.sessions).toEqual([]);
    expect(planOf(r, choiceForBranch(r.branches[1])).sessions.map((s) => s.session_id)).toEqual(["s-feat"]);
    expect(planOf(r, choiceForBranch(r.branches[0], "w1:p1")).sessions.map((s) => s.pane_id)).toEqual(["w1:p1"]);
  });

  it("a busy session refuses the transfer with its name", () => {
    expect(busyOf(planOf(repo(), everything()))).toBeNull();
    const busy = planOf(repo(fixture("working")), everything());
    expect(busyOf(busy)).toBe("feat chat is busy — wait until it is idle (or stop it) and try again");
    const waiting = planOf(repo(fixture("blocked")), { whole: false, branches: new Set(["w1"]), sessions: new Set() });
    expect(busyOf(waiting)).toBeNull();
  });

  it("a move says once what leaves the source", () => {
    const r = repo();
    expect(removalText(planOf(r, everything()), "laptop", "vdi")).toBe("This removes the repository atlas, all its worktrees and sessions from laptop after it arrives on vdi.");
    expect(removalText(planOf(r, choiceForBranch(r.branches[1])), "laptop", "vdi")).toBe("This removes the worktree feat/x and its branch and 1 session from laptop after it arrives on vdi.");
    expect(removalText(planOf(r, choiceForBranch(r.branches[0], "w1:p1")), "laptop", "vdi")).toBe("This removes 1 session from laptop after it arrives on vdi (the main checkout stays).");
    expect(removalText(planOf(r, { whole: false, branches: new Set(["w1"]), sessions: new Set() }), "laptop", "vdi")).toMatch(/^Nothing is removed from laptop/);
    expect(removeParams(planOf(r, choiceForBranch(r.branches[1])))).toEqual({
      kind: "repo", repo: "C:\\w\\atlas", whole: false, worktrees: [{ path: "C:\\wt\\atlas\\feat" }], sessions: ["s-feat"],
    });
  });
});

describe("clone addresses", () => {
  it("git addresses are recognised, paths and names are not", () => {
    for (const u of ["https://github.com/o/atlas.git", "git@github.com:o/atlas.git", "ssh://git@example.com:2222/o/r", "git://example.com/r", "me@example.com:repos/r"]) expect(isGitUrl(u), u).toBe(true);
    for (const u of ["", "atlas", "~/atlas", "C:\\Workspace\\atlas", "/home/me/atlas", "https://github.com", "git@github.com:", "a b@c:d"]) expect(isGitUrl(u), u).toBe(false);
  });
  it("the folder is named after the repository", () => {
    expect(repoNameOf("https://github.com/o/atlas.git")).toBe("atlas");
    expect(repoNameOf("git@github.com:o/notes")).toBe("notes");
    expect(repoNameOf("https://example.com/a/b/")).toBe("b");
  });
  it("GitHub addresses in every form", () => {
    for (const u of ["https://github.com/o/r", "git@github.com:o/r.git", "ssh://git@github.com/o/r", "https://me@github.com/o/r"]) expect(isGitHub(u), u).toBe(true);
    for (const u of ["https://gitlab.com/o/r", "git@example.com:o/r", "https://notgithub.com/o/r"]) expect(isGitHub(u), u).toBe(false);
  });
  it("progress reads as phase and percent", () => {
    expect(cloneLine({ phase: "Receiving objects", percent: 45 })).toBe("Receiving objects · 45%");
    expect(cloneLine({ phase: "", percent: null })).toBe("Cloning…");
  });
});
