import { describe as suite, expect, it } from "vitest";
import type { ArchiveEntry } from "./api";
import { describe, matches, nameOf, resumable, shortId, sorted, when, worktreeRemovable } from "./archive";

const base = (over: Partial<ArchiveEntry> = {}): ArchiveEntry => ({
  id: "7f0cff8f-729c-4907-b694-6f5ace18360e", source: "herdr", label: "auto-recover", repo_name: "Atlas-Tools", branch: "auto-recover",
  cwd: "C:\\Users\\me\\.herdr\\worktrees\\Atlas-Tools\\auto-recover", repo_root: "\\\\?\\C:\\Workspace\\Atlas-Tools", linked_worktree: true,
  sessions: [{ tab_label: "1", agent: "claude", session_id: "7f0cff8f-729c-4907-b694-6f5ace18360e", cwd: "x", resumable: true }, { tab_label: "2", agent: null, session_id: null, cwd: "x", resumable: false }],
  last_active_ms: 1_700_000_000_000, prompt: "почини мониторинг", prompts: 3,
  summary: null, summary_state: "pending", status: "closed", ...over,
});

suite("archive helpers", () => {
  it("shortId prefers the primary Claude session id", () => {
    expect(shortId(base())).toBe("7f0cff8f");
    expect(shortId(base({ id: "1a2b3c4d5e", sessions: [] }))).toBe("1a2b3c4d");
  });
  it("nameOf shows repo › branch under the label, or the cwd when no repo", () => {
    expect(nameOf(base())).toEqual({ title: "auto-recover", sub: "Atlas-Tools › auto-recover" });
    expect(nameOf(base({ repo_name: null, branch: null })).sub).toMatch(/auto-recover$/);
    expect(nameOf(base({ label: "Atlas", repo_name: "Atlas", branch: null })).sub).toMatch(/^C:/);
  });
  it("describe: summary when ready, otherwise the first prompt, with the writing/failed note", () => {
    expect(describe(base({ summary_state: "ready", summary: "Чинили X" })).text).toBe("Чинили X");
    expect(describe(base())).toEqual({ text: "почини мониторинг  ▞ writing…", state: "pending" });
    expect(describe(base({ summary_state: "failed", summary_error: "no claude" })).text).toContain("no claude");
    expect(describe(base({ summary_state: "none" }))).toEqual({ text: "почини мониторинг", state: "prompt" });
    expect(describe(base({ summary_state: "none", prompt: null })).state).toBe("none");
  });
  it("resumable counts sessions whose transcript still exists", () => {
    expect(resumable(base())).toBe(1);
    expect(resumable(base({ sessions: [{ tab_label: "", session_id: "x", cwd: "c", resumable: false }] }))).toBe(0);
  });
  it("worktreeRemovable: linked worktree from Herdr, or a cwd inside a worktrees folder; never path==root", () => {
    expect(worktreeRemovable(base())).toBe(true);
    expect(worktreeRemovable(base({ linked_worktree: false, repo_root: null }))).toBe(true);
    expect(worktreeRemovable(base({ linked_worktree: false, repo_root: null, cwd: "C:\\Workspace\\Atlas-Tools" }))).toBe(false);
    expect(worktreeRemovable(base({ cwd: "C:/Workspace/Atlas-Tools/" }))).toBe(false);
  });
  it("when: today shows the time, otherwise dd.mm hh:mm", () => {
    const now = new Date(2026, 10, 16, 17, 30).getTime();
    expect(when(new Date(2026, 10, 16, 9, 5).getTime(), now)).toBe("today 09:05");
    expect(when(new Date(2026, 10, 2, 23, 59).getTime(), now)).toBe("02.11 23:59");
  });
  it("sorted puts closed before restored, newest activity first within each", () => {
    const a = base({ id: "a", last_active_ms: 1 }), b = base({ id: "b", last_active_ms: 2 }), r = base({ id: "r", last_active_ms: 3, status: "restored" });
    expect(sorted([r, a, b]).map((e) => e.id)).toEqual(["b", "a", "r"]);
  });
  it("matches searches every word across label, repo, branch, cwd, prompt, summary and id", () => {
    expect(matches(base(), "atlas recover")).toBe(true);
    expect(matches(base(), "мониторинг")).toBe(true);
    expect(matches(base(), "7f0cff8f")).toBe(true);
    expect(matches(base(), "nope")).toBe(false);
    expect(matches(base(), "  ")).toBe(true);
  });
});
