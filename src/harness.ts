import type { ArchiveEntry, HarnessRepo, HarnessItem, RepoGroup } from "./api";
import { placement } from "./archive-tree";

/** A repository as the harness scan wants it: key (lowercased name), name, checkout root. */
export interface RepoRef { key: string; name: string; root: string }

/**
 * The repositories the archive knows, with the root checkout path when one is
 * known (a root session's cwd, or `repo_root` of a linked worktree). Repos
 * known only through worktree paths without `repo_root` have no root and are
 * left out — the harness lives in the main checkout. Newest activity first.
 */
export function harnessRepos(entries: ArchiveEntry[]): RepoRef[] {
  const seen = new Map<string, { ref: RepoRef; last: number; viaRoot: boolean }>();
  for (const e of entries) {
    const p = placement(e);
    const root = p.root ? p.path : e.repo_root ? e.repo_root.replace(/^\\\\\?\\/, "").replace(/[\\/]+/g, "/").replace(/\/$/, "") : null;
    if (!root) continue;
    const have = seen.get(p.repoKey);
    const last = e.last_active_ms ?? 0;
    if (!have || (p.root && !have.viaRoot) || (p.root === have.viaRoot && last > have.last)) {
      seen.set(p.repoKey, { ref: { key: p.repoKey, name: p.repo, root }, last: Math.max(last, have?.last ?? 0), viaRoot: p.root || !!have?.viaRoot });
    } else if (last > have.last) have.last = last;
  }
  return [...seen.values()].sort((a, b) => b.last - a.last).map((x) => x.ref);
}

export interface HarnessSection { group: RepoGroup | null; repos: HarnessRepo[] }

/** Personal is never grouped and always first; the rest follow the archive's groups (main first). */
export function sectionHarness(repos: HarnessRepo[], groups: RepoGroup[]): { personal: HarnessRepo | null; sections: HarnessSection[] } {
  const personal = repos.find((r) => r.personal) ?? null;
  const owner = new Map<string, RepoGroup>();
  for (const g of groups) for (const r of g.repos) if (!owner.has(r.toLowerCase())) owner.set(r.toLowerCase(), g);
  const main: HarnessSection = { group: null, repos: [] };
  const byId = new Map<string, HarnessSection>(groups.map((g) => [g.id, { group: g, repos: [] }]));
  for (const r of repos) {
    if (r.personal) continue;
    const g = owner.get(r.key);
    (g ? byId.get(g.id)! : main).repos.push(r);
  }
  return { personal, sections: [main, ...groups.map((g) => byId.get(g.id)!)] };
}

export const KIND_LABEL: Record<string, string> = {
  claude_md: "INSTRUCTIONS", settings: "SETTINGS", skill: "SKILLS", agent: "AGENTS", command: "COMMANDS", rule: "RULES", mcp: "MCP", hook: "HOOKS",
};
const KIND_ORDER = ["claude_md", "settings", "mcp", "skill", "agent", "command", "rule", "hook"];

/** Items of one repo by kind, in a fixed order, kinds without items omitted; inside a kind by first tag, then name. */
export function byKind(items: HarnessItem[]): { kind: string; label: string; items: HarnessItem[] }[] {
  const m = new Map<string, HarnessItem[]>();
  for (const i of items) (m.get(i.kind) ?? m.set(i.kind, []).get(i.kind)!).push(i);
  const order = (i: HarnessItem) => (i.tags[0] ?? "\uffff") + "\u0000" + i.name.toLowerCase();
  return KIND_ORDER.filter((k) => m.has(k)).map((k) => ({ kind: k, label: KIND_LABEL[k] ?? k.toUpperCase(), items: [...m.get(k)!].sort((a, b) => order(a).localeCompare(order(b))) }));
}

/** Tags used across a repo's items with their counts, most used first. */
export function tagCounts(items: HarnessItem[]): { tag: string; n: number }[] {
  const m = new Map<string, number>();
  for (const i of items) for (const t of i.tags) m.set(t, (m.get(t) ?? 0) + 1);
  return [...m.entries()].map(([tag, n]) => ({ tag, n })).sort((a, b) => b.n - a.n || a.tag.localeCompare(b.tag));
}

/** Items CLASSIFY would still tag (markdown instructions without tags). */
export const untagged = (items: HarnessItem[]) => items.filter((i) => ["skill", "agent", "command", "rule"].includes(i.kind) && /\.md$/i.test(i.path) && !i.tags.length).length;

/**
 * Filter repos (and their items): `query` words match repo name, item name,
 * kind or description (never tags); `tags` (selected chips) keep items that
 * carry any of them. A repo whose name matches keeps all its items, still
 * narrowed by the selected tags.
 */
export function matchHarness(repos: HarnessRepo[], query: string, tags: ReadonlySet<string> = new Set()): HarnessRepo[] {
  const words = query.toLowerCase().split(/\s+/).filter(Boolean);
  if (!words.length && !tags.size) return repos;
  const tagged = (i: HarnessItem) => !tags.size || i.tags.some((t) => tags.has(t));
  const hay = (i: HarnessItem) => `${i.kind} ${KIND_LABEL[i.kind] ?? ""} ${i.name} ${i.description ?? ""}`.toLowerCase();
  return repos
    .map((r) => {
      const repoHit = words.every((w) => r.name.toLowerCase().includes(w));
      const items = r.items.filter((i) => tagged(i) && (repoHit || words.every((w) => hay(i).includes(w))));
      return (repoHit && !tags.size) || items.length ? { ...r, items } : null;
    })
    .filter((r): r is HarnessRepo => r !== null);
}
