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

/**
 * Pure. The harness item a hack runs on a server, if that server's harness
 * has it: the same `/name` and kind, personal or in the repository of the
 * same name. `undefined` while the harness is not read; `null` when missing.
 */
export function hackItemIn(hack: Hack, harness: HarnessRepo[] | undefined): HarnessItem | null | undefined {
  if (!harness) return undefined;
  const repos = harness.filter((r) => (hack.source === "personal" ? r.personal : !r.personal && r.name === hack.repo_name));
  for (const r of repos) {
    const it = hackable(r.items).find((i) => i.kind === hack.kind && slashName(i) === hack.name);
    if (it) return it;
  }
  return null;
}

/**
 * Pure. COPY TO / MOVE TO for a pinned hack: the hack lands on server `to`
 * (pointing at that server's file when its harness has the same entry). A
 * copy is a new hack next to the old one; a move re-homes it. A hack the
 * target already has (same `/name`, kind and source) is not doubled.
 */
export function transferHack(hacks: Hack[], id: string, to: string, move: boolean, target: HarnessRepo[] | undefined): Hack[] {
  const h = hacks.find((x) => x.id === id);
  if (!h) return hacks;
  const item = hackItemIn(h, target);
  const there: Hack = { ...h, server: to, path: item?.path ?? h.path, description: item?.description ?? h.description };
  const twin = (x: Hack) => x.id !== h.id && x.server === to && x.name === h.name && x.kind === h.kind && x.source === h.source && (x.repo_name ?? null) === (h.repo_name ?? null);
  if (move) return hacks.some(twin) ? hacks.filter((x) => x.id !== h.id) : hacks.map((x) => (x.id === h.id ? there : x));
  if (hacks.some(twin)) return hacks;
  const i = hacks.indexOf(h);
  return [...hacks.slice(0, i + 1), { ...there, id: `${h.id}-${to.replace(/[^a-z0-9]/gi, "").slice(-8)}${Date.now().toString(36)}` }, ...hacks.slice(i + 1)];
}
