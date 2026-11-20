import { describe, expect, it } from "vitest";
import type { ArchiveEntry, HarnessRepo } from "./api";
import { byKind, harnessRepos, matchHarness, sectionHarness, tagCounts, untagged } from "./harness";

const entry = (o: Partial<ArchiveEntry>): ArchiveEntry => ({
  id: "x", source: "herdr", label: "", repo_name: null, branch: null, cwd: "", repo_root: null, linked_worktree: false, sessions: [],
  last_active_ms: 0, prompt: null, prompts: [], summary: null, summary_state: "none", summary_error: null, status: "closed", restored_at_ms: null, restored_workspace_id: null,
  ...o,
} as ArchiveEntry);

describe("harnessRepos", () => {
  it("one repo per key with its main checkout, newest first; root sessions beat worktree-derived roots", () => {
    const repos = harnessRepos([
      entry({ cwd: "C:/Users/me/.herdr/worktrees/Elly2/feature-x", repo_root: "\\\\?\\C:\\Workspace\\repos\\Elly2", repo_name: "Elly2", linked_worktree: true, last_active_ms: 50 }),
      entry({ cwd: "C:/Workspace/repos/Elly2", repo_root: "C:/Workspace/repos/Elly2", repo_name: "Elly2", last_active_ms: 10 }),
      entry({ cwd: "C:/Workspace/5.agents/netrunner-deck", last_active_ms: 99 }),
      entry({ cwd: "C:/Users/me/.herdr/worktrees/Orphan/wt-1", last_active_ms: 70 }),
    ]);
    expect(repos).toEqual([
      { key: "netrunner-deck", name: "netrunner-deck", root: "C:/Workspace/5.agents/netrunner-deck" },
      { key: "elly2", name: "Elly2", root: "C:/Workspace/repos/Elly2" },
    ]);
  });
});

const hr = (key: string, personal = false, items: HarnessRepo["items"] = []): HarnessRepo => ({ key, name: key, root: "/" + key, personal, exists: true, items });

describe("sectionHarness", () => {
  it("personal is pinned first; the rest follow the archive groups", () => {
    const { personal, sections } = sectionHarness([hr("personal", true), hr("a"), hr("b"), hr("c")], [{ id: "g1", name: "Work", repos: ["B", "c"], hidden: false }]);
    expect(personal?.key).toBe("personal");
    expect(sections.map((s) => [s.group?.name ?? "main", s.repos.map((r) => r.key)])).toEqual([["main", ["a"]], ["Work", ["b", "c"]]]);
  });
});

describe("byKind / matchHarness", () => {
  const items = [
    { kind: "skill", name: "deploy", path: "/a/.claude/skills/deploy/SKILL.md", size: 1, mtime_ms: 0, local: false, tags: ["devops"], description: "Ship a release" },
    { kind: "skill", name: "analyze", path: "/a/.claude/skills/analyze/SKILL.md", size: 1, mtime_ms: 0, local: false, tags: ["analytics", "sql"] },
    { kind: "skill", name: "zzz", path: "/a/.claude/skills/zzz/SKILL.md", size: 1, mtime_ms: 0, local: false, tags: [] },
    { kind: "claude_md", name: "CLAUDE.md", path: "/a/CLAUDE.md", size: 1, mtime_ms: 0, local: false, tags: [] },
    { kind: "settings", name: "settings.local.json", path: "/a/.claude/settings.local.json", size: 1, mtime_ms: 0, local: true, tags: [] },
  ];
  it("groups items in a fixed kind order; inside a kind tagged first, by first tag then name", () => {
    expect(byKind(items).map((g) => [g.label, g.items.map((i) => i.name)])).toEqual([["INSTRUCTIONS", ["CLAUDE.md"]], ["SETTINGS", ["settings.local.json"]], ["SKILLS", ["analyze", "deploy", "zzz"]]]);
  });
  it("text filters by repo name, item, kind or description — not tags; selected tags narrow by any-of", () => {
    const repos = [hr("alpha", false, items), hr("beta", false, items)];
    expect(matchHarness(repos, "alp").map((r) => [r.key, r.items.length])).toEqual([["alpha", 5]]);
    expect(matchHarness(repos, "skill dep").map((r) => [r.key, r.items.map((i) => i.name)])).toEqual([["alpha", ["deploy"]], ["beta", ["deploy"]]]);
    expect(matchHarness(repos, "release").map((r) => r.items.map((i) => i.name))).toEqual([["deploy"], ["deploy"]]);
    expect(matchHarness(repos, "sql")).toEqual([]);
    expect(matchHarness(repos, "", new Set(["sql", "devops"])).map((r) => r.items.map((i) => i.name))).toEqual([["deploy", "analyze"], ["deploy", "analyze"]]);
    expect(matchHarness(repos, "alpha", new Set(["sql"])).map((r) => [r.key, r.items.map((i) => i.name)])).toEqual([["alpha", ["analyze"]]]);
    expect(matchHarness(repos, "")).toBe(repos);
  });
  it("counts tags and the items CLASSIFY would still tag", () => {
    expect(tagCounts(items)).toEqual([{ tag: "analytics", n: 1 }, { tag: "devops", n: 1 }, { tag: "sql", n: 1 }]);
    expect(untagged(items)).toBe(1);
  });
});
