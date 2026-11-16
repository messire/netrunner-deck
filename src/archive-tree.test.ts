import { describe, expect, it } from "vitest";
import type { ArchiveEntry } from "./api";
import { buildArchiveTree, deletePlan, placement } from "./archive-tree";

const e = (over: Partial<ArchiveEntry>): ArchiveEntry => ({
  id: "x", source: "claude", label: "L", cwd: "C:\\x", linked_worktree: false, sessions: [], last_active_ms: 1, prompts: 1,
  summary_state: "none", status: "closed", ...over,
});

describe("placement", () => {
  it("Herdr linked worktree: repo from repo_name, worktree from branch", () => {
    const p = placement(e({ source: "herdr", repo_name: "Elly2-IR", repo_root: "\\\\?\\C:\\Workspace\\Elly2-IR", branch: "auto-recover", linked_worktree: true, cwd: "C:/Users/me/.herdr/worktrees/Elly2-IR/auto-recover" }));
    expect(p).toMatchObject({ repo: "Elly2-IR", worktree: "auto-recover", root: false, linked: true });
  });
  it("Herdr primary checkout → root worktree at repo_root", () => {
    const p = placement(e({ source: "herdr", repo_name: "Elly2", repo_root: "\\\\?\\C:\\Workspace\\Elly2", linked_worktree: false, cwd: "C:/Workspace/Elly2" }));
    expect(p).toMatchObject({ repo: "Elly2", worktree: "root", root: true, linked: false, path: "C:/Workspace/Elly2" });
  });
  it("cwd under .herdr/worktrees/<repo>/<wt> (even deeper) → that worktree", () => {
    const p = placement(e({ cwd: "C:\\Users\\me\\.herdr\\worktrees\\Elly2-IR\\b1-tree\\src\\sub" }));
    expect(p).toMatchObject({ repo: "Elly2-IR", worktree: "b1-tree", linked: true, path: "C:/Users/me/.herdr/worktrees/Elly2-IR/b1-tree" });
  });
  it("cwd under <repo>/.claude/worktrees/<wt> → repo is the folder above .claude", () => {
    const p = placement(e({ cwd: "C:\\Workspace\\repos\\Elly2\\.claude\\worktrees\\869eaq0qu-attr" }));
    expect(p).toMatchObject({ repo: "Elly2", worktree: "869eaq0qu-attr", linked: true });
  });
  it("plain folder → repo named after it, root worktree", () => {
    expect(placement(e({ cwd: "C:\\Workspace\\5.agents\\netrunner-deck" }))).toMatchObject({ repo: "netrunner-deck", worktree: "root", root: true, linked: false });
  });
});

describe("buildArchiveTree / deletePlan", () => {
  const entries = [
    e({ id: "a", cwd: "C:\\Workspace\\Elly2", last_active_ms: 5 }),
    e({ id: "b", cwd: "C:\\Users\\me\\.herdr\\worktrees\\Elly2\\wt1", last_active_ms: 9 }),
    e({ id: "c", cwd: "C:\\Users\\me\\.herdr\\worktrees\\Elly2\\wt1", last_active_ms: 2, status: "restored" }),
    e({ id: "d", cwd: "C:\\Users\\me\\.herdr\\worktrees\\Elly2\\wt1", last_active_ms: 3 }),
    e({ id: "z", cwd: "D:\\other", last_active_ms: 7, prompt: "needle" }),
  ];
  it("groups repo › worktree › session, root first, newest activity first, restored last", () => {
    const tree = buildArchiveTree(entries);
    expect(tree.map((r) => r.name)).toEqual(["Elly2", "other"]);
    const elly = tree[0];
    expect(elly.sessions).toBe(4);
    expect(elly.worktrees.map((w) => w.name)).toEqual(["root", "wt1"]);
    expect(elly.worktrees[1].sessions.map((s) => s.entry.id)).toEqual(["b", "d", "c"]);
    expect(elly.worktrees[1].lastActive).toBe(9);
  });
  it("case-insensitive repo keys merge Elly2 and elly2", () => {
    const tree = buildArchiveTree([e({ id: "1", cwd: "C:\\a\\Elly2" }), e({ id: "2", cwd: "c:\\A\\elly2" })]);
    expect(tree).toHaveLength(1);
  });
  it("query filters sessions before grouping", () => {
    const tree = buildArchiveTree(entries, "needle");
    expect(tree.map((r) => r.name)).toEqual(["other"]);
  });
  it("deletePlan: every session under the node; the linked worktree goes with its last session, root never", () => {
    const tree = buildArchiveTree(entries);
    expect(deletePlan(tree[0].worktrees[1])).toEqual([{ id: "b", removeWorktree: false }, { id: "d", removeWorktree: false }, { id: "c", removeWorktree: true }]);
    expect(deletePlan(tree[0].worktrees[0])).toEqual([{ id: "a", removeWorktree: false }]);
    expect(deletePlan(tree[0]).map((x) => x.id)).toEqual(["a", "b", "d", "c"]);
  });
});
