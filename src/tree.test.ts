import { describe, expect, it } from "vitest";
import type { Agent, Pane, Tab, Tree, Workspace } from "./api";
import { buildTree, contains, ctxPct, defaultOpen, firstPane, loadOpen, rollup, saveOpen, spaces, waiting } from "./tree";

const ws = (id: string, label: string, extra: Partial<Workspace> = {}): Workspace => ({
  workspace_id: id, number: 1, label, focused: false, pane_count: 1, tab_count: 1, active_tab_id: `${id}:t1`, agent_status: "idle", ...extra,
});
const tab = (id: string, workspace_id: string, label: string, extra: Partial<Tab> = {}): Tab => ({
  tab_id: id, workspace_id, number: 1, label, focused: false, pane_count: 1, agent_status: "idle", ...extra,
});
const pane = (id: string, workspace_id: string, tab_id: string, extra: Partial<Pane> = {}): Pane => ({
  pane_id: id, terminal_id: `term-${id}`, workspace_id, tab_id, focused: false, agent_status: "unknown", ...extra,
});
const agent = (p: Pane, extra: Partial<Agent> = {}): Agent => ({
  terminal_id: p.terminal_id, agent: "claude", agent_status: "idle", workspace_id: p.workspace_id, tab_id: p.tab_id, pane_id: p.pane_id, focused: false, ...extra,
});

const elly = { repo_key: "\\\\?\\C:\\Elly2\\.git", repo_name: "Elly2" };

/** Two repos, one standalone space, one split tab with a shell pane. */
function fixture(): Tree {
  const p1 = pane("w1:p1", "w1", "w1:t1", { agent: "claude", focused: true });
  const p2 = pane("w1:p2", "w1", "w1:t2", { agent: "codex" });
  const p3 = pane("w1:p3", "w1", "w1:t2", { title: "pwsh" });
  const p4 = pane("w2:p1", "w2", "w2:t1", { agent: "claude" });
  const p5 = pane("w3:p1", "w3", "w3:t1", { agent: "claude" });
  const p6 = pane("w4:p1", "w4", "w4:t1", { title: "pwsh" });
  return {
    workspaces: [
      ws("w1", "Elly2", { worktree: { ...elly, is_linked_worktree: false }, agent_status: "working", tab_count: 2 }),
      ws("w2", "attr-perf", { worktree: { ...elly, is_linked_worktree: true }, agent_status: "blocked" }),
      ws("w3", "IR", { worktree: { repo_key: "ir", repo_name: "Elly2-IncidentResponder", is_linked_worktree: false } }),
      ws("w4", "scratch"),
    ],
    tabs: [
      tab("w1:t1", "w1", "Elly2", { agent_status: "working", focused: true }),
      tab("w1:t2", "w1", "review", { pane_count: 2 }),
      tab("w2:t1", "w2", "main", { agent_status: "blocked" }),
      tab("w3:t1", "w3", "1"),
      tab("w4:t1", "w4", "1"),
    ],
    panes: [p1, p2, p3, p4, p5, p6],
    agents: [
      agent(p1, { agent_status: "working", tokens: { ctx: "42" } }),
      agent(p2, { agent: "codex", display_agent: "Codex" }),
      agent(p4, { agent_status: "blocked" }),
      agent(p5, { name: "triage" }),
    ],
  };
}

describe("buildTree", () => {
  it("groups worktree spaces under their repo and keeps plain spaces at the root, in server order", () => {
    const roots = buildTree(fixture());
    expect(roots.map((r) => (r.kind === "repo" ? `repo:${r.label}` : `space:${r.workspace.label}`)))
      .toEqual(["repo:Elly2", "repo:Elly2-IncidentResponder", "space:scratch"]);
    expect(spaces(roots).map((s) => s.workspace.label)).toEqual(["Elly2", "attr-perf", "IR", "scratch"]);
  });

  it("marks the primary checkout as root and linked worktrees as linked", () => {
    const [elly] = buildTree(fixture());
    if (elly.kind !== "repo") throw new Error("expected repo");
    expect(elly.spaces.map((s) => [s.root, s.linked])).toEqual([[true, false], [false, true]]);
  });

  it("collapses a one-pane tab into a session and keeps split tabs nested", () => {
    const [elly] = buildTree(fixture());
    if (elly.kind !== "repo") throw new Error("expected repo");
    const [t1, t2] = elly.spaces[0].tabs;
    expect(t1.single?.id).toBe("w1:p1");
    expect(t2.single).toBeNull();
    expect(t2.sessions.map((s) => [s.label, s.agentKind, s.status])).toEqual([["Codex", "Codex", "idle"], ["pwsh", null, "unknown"]]);
  });

  it("counts agents (not shells) per space and repo and rolls repo status up from spaces", () => {
    const roots = buildTree(fixture());
    const elly = roots[0];
    if (elly.kind !== "repo") throw new Error("expected repo");
    expect(elly.spaces.map((s) => s.agents)).toEqual([2, 1]);
    expect(elly.agents).toBe(3);
    expect(elly.status).toBe("blocked");
    const scratch = roots[2];
    expect(scratch.kind === "space" && scratch.agents).toBe(0);
  });

  it("prefers the agent's own name for a session label", () => {
    const roots = buildTree(fixture());
    const ir = roots[1];
    if (ir.kind !== "repo") throw new Error("expected repo");
    expect(ir.spaces[0].tabs[0].single?.label).toBe("triage");
  });

  it("returns nothing without a tree", () => {
    expect(buildTree(undefined)).toEqual([]);
  });
});

describe("rollup", () => {
  it("picks the most urgent status and falls back to unknown", () => {
    expect(rollup(["idle", "working", "done"])).toBe("working");
    expect(rollup(["idle", "blocked", "working"])).toBe("blocked");
    expect(rollup([])).toBe("unknown");
    expect(rollup(["unknown", "done"])).toBe("done");
  });
});

describe("contains / defaultOpen / firstPane", () => {
  it("finds a pane at any depth", () => {
    const roots = buildTree(fixture());
    expect(contains(roots[0], "w1:p3")).toBe(true);
    expect(contains(roots[1], "w1:p3")).toBe(false);
    expect(contains(roots[0], null)).toBe(false);
  });

  it("opens nodes that are busy, waiting or hold the selected pane; others start collapsed", () => {
    const roots = buildTree(fixture());
    const ir = roots[1];
    expect(defaultOpen(roots[0], null)).toBe(true); // blocked inside
    expect(defaultOpen(ir, null)).toBe(false);
    expect(defaultOpen(ir, "w3:p1")).toBe(true);
    if (ir.kind !== "repo") throw new Error("expected repo");
    expect(defaultOpen(ir.spaces[0].tabs[0], "w3:p1")).toBe(true);
  });

  it("picks the focused pane first, else the first pane", () => {
    const roots = buildTree(fixture());
    expect(firstPane(roots[0])?.pane_id).toBe("w1:p1");
    expect(firstPane(roots[1])?.pane_id).toBe("w3:p1");
    const elly = roots[0];
    if (elly.kind !== "repo") throw new Error("expected repo");
    expect(firstPane(elly.spaces[0].tabs[1])?.pane_id).toBe("w1:p2");
  });
});

describe("waiting", () => {
  it("lists blocked sessions with a repo / space / tab path", () => {
    const w = waiting(buildTree(fixture()));
    expect(w.map((x) => [x.session.id, x.path])).toEqual([["w2:p1", "Elly2 / attr-perf / main"]]);
  });

  it("drops repeated names (root space = repo, tab = space) and adds the session for split tabs", () => {
    const t = fixture();
    t.agents = t.agents.map((a) => (a.pane_id === "w1:p1" || a.pane_id === "w1:p2" ? { ...a, agent_status: "blocked" } : a));
    const w = waiting(buildTree(t));
    expect(w.map((x) => x.path)).toEqual(["Elly2", "Elly2 / review / Codex", "Elly2 / attr-perf / main"]);
  });
});

describe("ctxPct / open state", () => {
  it("parses the ctx token or returns null", () => {
    expect(ctxPct({ ctx: "42" })).toBe(42);
    expect(ctxPct({ ctx: "n/a" })).toBeNull();
    expect(ctxPct(undefined)).toBeNull();
  });

  it("round-trips open overrides through storage and survives garbage", () => {
    const store = new Map<string, string>();
    const storage = { getItem: (k: string) => store.get(k) ?? null, setItem: (k: string, v: string) => { store.set(k, v); } };
    saveOpen(storage, { "repo:x": false, "w1": true });
    expect(loadOpen(storage)).toEqual({ "repo:x": false, "w1": true });
    store.set("deck.tree.open", "{not json");
    expect(loadOpen(storage)).toEqual({});
    expect(loadOpen(undefined)).toEqual({});
  });
});
