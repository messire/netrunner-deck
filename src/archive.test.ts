import { describe as suite, expect, it } from "vitest";
import type { ArchiveEntry } from "./api";
import { describe, nameOf, resumable, shortId, sorted, when, worktreeRemovable } from "./archive";

const base = (over: Partial<ArchiveEntry> = {}): ArchiveEntry => ({
  id: "1a2b3c4d5e", archived_at_ms: 1_700_000_000_000, label: "auto-recover", repo_name: "Elly2-IR", branch: "auto-recover",
  cwd: "C:\\Users\\me\\.herdr\\worktrees\\Elly2-IR\\auto-recover", repo_root: "\\\\?\\C:\\Workspace\\Elly2-IR", linked_worktree: true,
  sessions: [{ tab_label: "1", agent: "claude", session_id: "7f0cff8f-729c-4907-b694-6f5ace18360e", cwd: "x" }, { tab_label: "2", agent: null, session_id: null, cwd: "x" }],
  summary: null, summary_state: "pending", status: "archived", ...over,
});

suite("archive helpers", () => {
  it("shortId prefers the primary Claude session id", () => {
    expect(shortId(base())).toBe("7f0cff8f");
    expect(shortId(base({ sessions: [] }))).toBe("1a2b3c4d");
  });
  it("nameOf shows repo › branch under the label, or the cwd when no repo", () => {
    expect(nameOf(base())).toEqual({ title: "auto-recover", sub: "Elly2-IR › auto-recover" });
    expect(nameOf(base({ repo_name: null, branch: null })).sub).toMatch(/auto-recover$/);
    expect(nameOf(base({ label: "Elly2", repo_name: "Elly2", branch: null })).sub).toMatch(/^C:/);
  });
  it("describe follows summary_state", () => {
    expect(describe(base({ summary_state: "ready", summary: "Чинили X" })).text).toBe("Чинили X");
    expect(describe(base()).state).toBe("pending");
    expect(describe(base({ summary_state: "failed", summary_error: "no claude" })).text).toContain("no claude");
    expect(describe(base({ summary_state: "none" })).state).toBe("none");
  });
  it("resumable counts only sessions with an id", () => {
    expect(resumable(base())).toBe(1);
  });
  it("worktreeRemovable refuses the main checkout and path==root (ignoring \\\\?\\ and slashes)", () => {
    expect(worktreeRemovable(base())).toBe(true);
    expect(worktreeRemovable(base({ linked_worktree: false }))).toBe(false);
    expect(worktreeRemovable(base({ repo_root: null }))).toBe(false);
    expect(worktreeRemovable(base({ cwd: "C:/Workspace/Elly2-IR/" }))).toBe(false);
  });
  it("when: today shows the time, otherwise dd.mm hh:mm", () => {
    const now = new Date(2026, 10, 16, 17, 30).getTime();
    expect(when(new Date(2026, 10, 16, 9, 5).getTime(), now)).toBe("today 09:05");
    expect(when(new Date(2026, 10, 2, 23, 59).getTime(), now)).toBe("02.11 23:59");
  });
  it("sorted puts archived before restored, newest first within each", () => {
    const a = base({ id: "a", archived_at_ms: 1 }), b = base({ id: "b", archived_at_ms: 2 }), r = base({ id: "r", archived_at_ms: 3, status: "restored" });
    expect(sorted([r, a, b]).map((e) => e.id)).toEqual(["b", "a", "r"]);
  });
});
