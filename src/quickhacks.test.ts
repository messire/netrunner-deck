import { describe, expect, it } from "vitest";
import type { HarnessRepo, WorktreeInfo } from "./api";
import { filterItems, hackFrom, hackPrompt, hackable, launchLabel, worktreeChoices } from "./quickhacks";

const item = (kind: string, name: string, description = "", argument_hint?: string) => ({ kind, name, path: `/${name}.md`, size: 1, mtime_ms: 0, local: false, tags: [], description, argument_hint });
const repo: HarnessRepo = { key: "elly2", name: "Elly2", root: "C:/r", personal: false, exists: true, items: [item("skill", "ir-feedback", "IR", "<period>"), item("command", "git/sync"), item("agent", "x"), item("claude_md", "CLAUDE.md")] };

describe("quickhacks", () => {
  it("only skills and commands are hackable; a hack remembers its source repo and hint", () => {
    expect(hackable(repo.items).map((i) => i.name)).toEqual(["ir-feedback", "git/sync"]);
    const h = hackFrom(repo, repo.items[1]);
    expect(h).toMatchObject({ name: "sync", kind: "command", source: "project", repo_key: "elly2", repo_name: "Elly2" });
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
