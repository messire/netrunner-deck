import type { ArchiveEntry } from "./api";

/**
 * Pure helpers for the ARCHIVE view (cold storage of closed spaces), kept out
 * of the component so the table's cells and guards are unit-testable.
 */

/** Short id shown in the table: the primary Claude session's first 8 hex chars, else the entry id. */
export function shortId(e: ArchiveEntry): string {
  const sid = e.sessions.find((s) => s.session_id)?.session_id;
  return (sid ?? e.id).slice(0, 8);
}

/** Space label plus repo/branch when the space was a worktree. */
export function nameOf(e: ArchiveEntry): { title: string; sub: string } {
  const repo = e.repo_name ? `${e.repo_name}${e.branch ? ` › ${e.branch}` : ""}` : "";
  const sub = repo && repo !== e.label ? repo : e.cwd;
  return { title: e.label, sub };
}

/**
 * What the DESCRIPTION cell shows: the shadow-written summary when there is
 * one, the first prompt of the session while there is not (or when writing
 * failed), and a note while the description is being written.
 */
export function describe(e: ArchiveEntry): { text: string; state: ArchiveEntry["summary_state"] | "prompt" } {
  if (e.summary_state === "ready" && e.summary) return { text: e.summary, state: "ready" };
  const prompt = e.prompt?.trim();
  if (e.summary_state === "pending") return { text: prompt ? `${prompt}  ▞ writing…` : "▞ writing the description… (shadow claude -p)", state: "pending" };
  if (e.summary_state === "failed") return { text: `${prompt ? prompt + "  " : ""}✕ description failed: ${e.summary_error ?? "unknown"}`, state: "failed" };
  if (prompt) return { text: prompt, state: "prompt" };
  return { text: "no prompt recorded", state: "none" };
}

/** How many sessions `claude --resume` can bring back. */
export function resumable(e: ArchiveEntry): number {
  return e.sessions.filter((s) => s.resumable).length;
}

/**
 * Whether the UI offers "+ WORKTREE": a linked worktree Herdr described, or a
 * session whose cwd sits in a worktree folder (`.herdr/worktrees`,
 * `.claude/worktrees`); Rust re-checks with git before removing anything.
 */
export function worktreeRemovable(e: ArchiveEntry): boolean {
  if (e.linked_worktree && e.repo_root) return norm(e.repo_root) !== norm(e.cwd);
  return /\/\.(herdr|claude)\/worktrees\/[^/]+/.test(norm(e.cwd));
}
const norm = (p: string) => p.replace(/^\\\\\?\\/, "").replace(/[\\/]+/g, "/").replace(/\/$/, "").toLowerCase();

export function when(ms: number, now = Date.now()): string {
  const d = new Date(ms);
  const sameDay = new Date(now).toDateString() === d.toDateString();
  const hm = d.toTimeString().slice(0, 5);
  if (sameDay) return `today ${hm}`;
  const dd = String(d.getDate()).padStart(2, "0"), mm = String(d.getMonth() + 1).padStart(2, "0");
  return `${dd}.${mm} ${hm}`;
}

/** Newest activity first; restored entries after closed ones so the restorable work is on top. */
export function sorted(entries: ArchiveEntry[]): ArchiveEntry[] {
  return [...entries].sort((a, b) => (a.status === b.status ? b.last_active_ms - a.last_active_ms : a.status === "closed" ? -1 : 1));
}

/** Filter by free text over label, repo, branch, cwd, prompt, summary and id. */
export function matches(e: ArchiveEntry, query: string): boolean {
  const q = query.trim().toLowerCase();
  if (!q) return true;
  const hay = [e.label, e.repo_name, e.branch, e.cwd, e.prompt, e.summary, e.id].filter(Boolean).join(" ").toLowerCase();
  return q.split(/\s+/).every((w) => hay.includes(w));
}
