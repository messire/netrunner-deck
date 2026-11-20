import type { HarnessRepo } from "./api";

/** One thing `/` can start: a skill or command from the harness, or a built-in. */
export interface SlashItem { name: string; kind: "skill" | "command" | "builtin"; description: string; source: string; args?: string }

/** Claude Code's own slash commands (the common ones; the CLI shows the full list on `/`). */
export const BUILTINS: SlashItem[] = [
  ["help", "show help and available commands"], ["clear", "clear conversation history"], ["compact", "compact the conversation, keep a summary"],
  ["context", "show what is in the context window"], ["cost", "token usage and cost of this session"], ["model", "choose the model"],
  ["resume", "resume another session"], ["rewind", "rewind the conversation / files to a checkpoint"], ["memory", "edit CLAUDE.md memory files"],
  ["init", "create a CLAUDE.md for this project"], ["review", "review a pull request"], ["status", "version, account, connectivity"],
  ["config", "open settings"], ["permissions", "manage tool permissions"], ["mcp", "manage MCP servers"], ["hooks", "manage hooks"],
  ["agents", "manage subagents"], ["add-dir", "add a working directory"], ["doctor", "check the installation"], ["export", "export the conversation"],
  ["login", "sign in"], ["logout", "sign out"], ["bug", "report a bug"], ["plan", "plan mode"], ["vim", "vim keybindings"],
].map(([name, description]) => ({ name, kind: "builtin" as const, description, source: "claude code" }));

const norm = (p: string) => p.replace(/^\\\\\?\\/, "").replace(/[\\/]+/g, "/").replace(/\/$/, "").toLowerCase();

/**
 * Slash items for a pane: skills and commands of the repository whose root
 * contains `cwd` (the deepest such root), the personal ones, then built-ins.
 * Nested commands (`git/sync.md`) answer to `/sync` (namespace in `source`).
 */
export function slashItems(repos: HarnessRepo[], cwd: string | null | undefined): SlashItem[] {
  const here = cwd ? norm(cwd) : "";
  const inRepo = repos
    .filter((r) => !r.personal && here && (here === norm(r.root) || here.startsWith(norm(r.root) + "/")))
    .sort((a, b) => norm(b.root).length - norm(a.root).length)[0];
  const personal = repos.find((r) => r.personal);
  const out: SlashItem[] = [];
  const seen = new Set<string>();
  for (const [repo, where] of [[inRepo, "project"], [personal, "user"]] as const) {
    for (const i of repo?.items ?? []) {
      if (i.kind !== "skill" && i.kind !== "command") continue;
      const parts = i.name.split("/");
      const name = parts[parts.length - 1];
      if (seen.has(name)) continue;
      seen.add(name);
      const ns = parts.length > 1 ? `:${parts.slice(0, -1).join(":")}` : "";
      out.push({ name, kind: i.kind, description: i.description ?? "", source: `${where}${ns}`, args: i.argument_hint ?? undefined });
    }
  }
  for (const b of BUILTINS) if (!seen.has(b.name)) out.push(b);
  return out;
}

/** The `/word` being typed at the start of the draft, or null (anything after a space is arguments: no popup). */
export function slashQuery(draft: string): string | null {
  const m = /^\/([\w:-]*)$/.exec(draft);
  return m ? m[1].toLowerCase() : null;
}

/** Items matching `query`: prefix matches first, then substring matches of the name, then description hits; at most `limit`. */
export function slashMatches(items: SlashItem[], query: string, limit = 12): SlashItem[] {
  const q = query.toLowerCase();
  const rank = (i: SlashItem) => (i.name.startsWith(q) ? 0 : i.name.includes(q) ? 1 : i.description.toLowerCase().includes(q) ? 2 : 3);
  return items
    .map((i) => ({ i, r: rank(i) }))
    .filter((x) => x.r < 3)
    .sort((a, b) => a.r - b.r || (a.i.kind === "builtin" ? 1 : 0) - (b.i.kind === "builtin" ? 1 : 0) || a.i.name.localeCompare(b.i.name))
    .slice(0, limit)
    .map((x) => x.i);
}

/** The item a typed command refers to (`/name` or `/name args…`), for the hint under the input. */
export function slashCurrent(items: SlashItem[], draft: string): { item: SlashItem; args: string } | null {
  const m = /^\/([\w:-]+)(?:\s+([\s\S]*))?$/.exec(draft);
  if (!m) return null;
  const item = items.find((i) => i.name === m[1].toLowerCase());
  return item ? { item, args: m[2] ?? "" } : null;
}
