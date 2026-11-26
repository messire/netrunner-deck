/**
 * PREFERENCES › CHAT: the model and permission mode every new chat starts
 * with. `null` is "CLI default": Deck leaves that one to Claude Code. A new
 * chat is a headless pane that appears after its server was first seen; Deck
 * sends it `deck.agent.set` once, as soon as the session takes it.
 */
export interface ChatDefaults { model: string | null; mode: string | null }

const KEY = "deck.chatDefaults";

export function loadChatDefaults(): ChatDefaults {
  try {
    const v = JSON.parse(localStorage.getItem(KEY) ?? "{}") as Partial<ChatDefaults>;
    return { model: typeof v.model === "string" && v.model ? v.model : null, mode: typeof v.mode === "string" && v.mode ? v.mode : null };
  } catch { return { model: null, mode: null }; }
}

export function saveChatDefaults(d: ChatDefaults) {
  try { localStorage.setItem(KEY, JSON.stringify(d)); } catch { /* storage unavailable */ }
}

/** Pure. The `deck.agent.set` params for the defaults; null when there is nothing to set. */
export function defaultsParams(d: ChatDefaults): Record<string, string> | null {
  const out: Record<string, string> = {};
  if (d.mode) out.permission_mode = d.mode;
  if (d.model) out.model = d.model;
  return Object.keys(out).length ? out : null;
}

/**
 * Pure. Panes that are new since the last look: present now, not seen before,
 * on a server that was already seen (a server's first tree is not "new").
 * Returns the new panes and the updated `seen` map (server → pane ids).
 */
export function newPanes(seen: ReadonlyMap<string, ReadonlySet<string>>, now: { pane: string; server: string }[]): { fresh: string[]; seen: Map<string, Set<string>> } {
  const next = new Map<string, Set<string>>();
  const fresh: string[] = [];
  for (const { pane, server } of now) {
    if (!next.has(server)) next.set(server, new Set());
    next.get(server)!.add(pane);
    const before = seen.get(server);
    if (before && !before.has(pane)) fresh.push(pane);
  }
  return { fresh, seen: next };
}

/** Pure. Model choices for the picker: the CLI's aliases, what sessions listed, and the saved value. */
export function modelChoices(listed: string[], saved: string | null): string[] {
  const all = ["opus", "sonnet", "haiku", ...listed.filter((m) => m && m !== "default"), ...(saved ? [saved] : [])];
  return [...new Set(all)];
}
