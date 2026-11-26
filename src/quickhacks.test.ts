import { describe, expect, it } from "vitest";
import type { Hack, HarnessRepo, WorktreeInfo } from "./api";
import { filterItems, hackFrom, hackItemIn, hackPrompt, hackable, launchLabel, transferHack, worktreeChoices } from "./quickhacks";

const item = (kind: string, name: string, description = "", argument_hint?: string) => ({ kind, name, path: `/${name}.md`, size: 1, mtime_ms: 0, local: false, tags: [], description, argument_hint });
const repo: HarnessRepo = { key: "atlas", name: "Atlas", root: "C:/r", personal: false, exists: true, items: [item("skill", "ir-feedback", "IR", "<period>"), item("command", "git/sync"), item("agent", "x"), item("claude_md", "CLAUDE.md")] };

describe("quickhacks", () => {
  it("only skills and commands are hackable; a hack remembers its source repo and hint", () => {
    expect(hackable(repo.items).map((i) => i.name)).toEqual(["ir-feedback", "git/sync"]);
    const h = hackFrom(repo, repo.items[1]);
    expect(h).toMatchObject({ name: "sync", kind: "command", source: "project", repo_key: "atlas", repo_name: "Atlas" });
    const p = hackFrom({ ...repo, personal: true }, repo.items[0]);
    expect(p).toMatchObject({ source: "personal", repo_key: null, argument_hint: "<period>" });
    expect(hackPrompt(p, " 7d ")).toBe("/ir-feedback 7d");
    expect(hackPrompt(p, "")).toBe("/ir-feedback");
  });
  it("filters by words; worktrees: main first, no bare/prunable", () => {
    expect(filterItems(repo.items, "ir").map((i) => i.name)).toEqual(["ir-feedback"]);
    const wts: WorktreeInfo[] = [
      { path: "C:/r/.herdr/b", branch: "feat-b", is_bare: false, is_detached: false, is_prunable: false, is_linked_worktree: true, label: "feat-b" },
      { path: "C:/r", branch: "main", is_bare: false, is_detached: false, is_prunable: false, is_linked_worktree: false, label: "main" },
      { path: "C:/r/.herdr/gone", branch: "gone", is_bare: false, is_detached: false, is_prunable: true, is_linked_worktree: true, label: "gone" },
      { path: "C:/r/.herdr/a", branch: "feat-a", is_bare: false, is_detached: false, is_prunable: false, is_linked_worktree: true, label: "feat-a" },
    ];
    expect(worktreeChoices(wts).map((w) => w.label)).toEqual(["main", "feat-a", "feat-b"]);
    expect(launchLabel(hackFrom(repo, repo.items[0]), wts[1])).toBe("/ir-feedback · main");
  });
});

describe("quickhacks COPY TO / MOVE TO", () => {
  const item = (name: string, kind: string, path: string) => ({ kind, name, path, size: 1, mtime_ms: 0, local: false, tags: [] });
  const there: HarnessRepo[] = [
    { key: "personal", name: "Personal", root: "/home/u/.claude", personal: true, exists: true, items: [item("ir-report", "skill", "/home/u/.claude/skills/ir-report/SKILL.md")] },
  ];
  const hack = (id: string, name: string, server: string | null): Hack => ({ id, name, kind: "skill", source: "personal", path: `C:/x/${name}`, server });
  const hacks = [hack("a", "ir-report", "local"), hack("b", "ir-feedback", "local")];

  it("finds the same entry in the target harness", () => {
    expect(hackItemIn(hacks[0], there)?.path).toBe("/home/u/.claude/skills/ir-report/SKILL.md");
    expect(hackItemIn(hacks[1], there)).toBeNull();
    expect(hackItemIn(hacks[1], undefined)).toBeUndefined();
  });

  it("copies next to the original, pointing at the target's file", () => {
    const out = transferHack(hacks, "a", "remote", false, there);
    expect(out.map((h) => h.server)).toEqual(["local", "remote", "local"]);
    expect(out[1].path).toBe("/home/u/.claude/skills/ir-report/SKILL.md");
    expect(out[1].id).not.toBe("a");
    expect(transferHack(out, "a", "remote", false, there)).toBe(out);
  });

  it("moves by re-homing, even when the target lacks the entry", () => {
    const out = transferHack(hacks, "b", "remote", true, there);
    expect(out.map((h) => [h.id, h.server])).toEqual([["a", "local"], ["b", "remote"]]);
    expect(out[1].path).toBe("C:/x/ir-feedback");
  });
});
