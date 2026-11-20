import type { Hack, HarnessItem, HarnessRepo, WorktreeInfo } from "./api";

/** Harness entries a hack can point at: skills and commands (the things `/name` runs). */
export const hackable = (items: HarnessItem[]) => items.filter((i) => i.kind === "skill" || i.kind === "command");

/** `/name` of an item: nested commands answer to their last path segment. */
export const slashName = (item: HarnessItem) => item.name.split("/").pop() ?? item.name;

/** A new hack from a harness item of `repo`. */
export function hackFrom(repo: HarnessRepo, item: HarnessItem): Hack {
  return {
    id: `h${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`,
    name: slashName(item), kind: item.kind,
    source: repo.personal ? "personal" : "project",
    repo_key: repo.personal ? null : repo.key, repo_name: repo.personal ? null : repo.name,
    path: item.path, description: item.description ?? null, argument_hint: item.argument_hint ?? null,
  };
}

/** Case-insensitive word filter over name + description. */
export function filterItems<T extends { name: string; description?: string | null }>(items: T[], query: string): T[] {
  const words = query.toLowerCase().split(/\s+/).filter(Boolean);
  if (!words.length) return items;
  return items.filter((i) => words.every((w) => `${i.name} ${i.description ?? ""}`.toLowerCase().includes(w)));
}

/** The prompt the new session starts with. */
export const hackPrompt = (hack: Hack, args: string) => `/${hack.name}${args.trim() ? ` ${args.trim()}` : ""}`;

/** Main checkout first, then linked worktrees by label; bare/prunable ones are not places to work. */
export function worktreeChoices(list: WorktreeInfo[]): WorktreeInfo[] {
  return list
    .filter((w) => !w.is_bare && !w.is_prunable)
    .sort((a, b) => Number(a.is_linked_worktree) - Number(b.is_linked_worktree) || a.label.localeCompare(b.label));
}

/** Tab label for a launched hack: `/name · branch`. */
export const launchLabel = (hack: Hack, wt: WorktreeInfo) => `/${hack.name} · ${wt.branch ?? wt.label}`;
