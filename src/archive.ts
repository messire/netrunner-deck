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

export function describe(e: ArchiveEntry): { text: string; state: ArchiveEntry["summary_state"] } {
  switch (e.summary_state) {
    case "ready": return { text: e.summary ?? "", state: "ready" };
    case "pending": return { text: "▞ writing the description… (shadow claude -p)", state: "pending" };
    case "failed": return { text: `description failed: ${e.summary_error ?? "unknown"}`, state: "failed" };
    default: return { text: "no Claude session in this space — nothing to resume, only the folder", state: "none" };
  }
}

/** How many sessions `claude --resume` can bring back. */
export function resumable(e: ArchiveEntry): number {
  return e.sessions.filter((s) => s.session_id).length;
}

/** Whether DELETE may also remove the checkout: only a linked worktree, never the repo itself. */
export function worktreeRemovable(e: ArchiveEntry): boolean {
  return e.linked_worktree && !!e.repo_root && norm(e.repo_root) !== norm(e.cwd);
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

/** Newest first; restored entries after archived ones so the restorable work is on top. */
export function sorted(entries: ArchiveEntry[]): ArchiveEntry[] {
  return [...entries].sort((a, b) => (a.status === b.status ? b.archived_at_ms - a.archived_at_ms : a.status === "archived" ? -1 : 1));
}
