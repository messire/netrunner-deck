/**
 * Several servers at once (docs/plan/DECK-SERVERS-UI.md). Pane, tab and
 * workspace ids repeat across servers (`w1:p1` exists on each), so the
 * webview never sees a bare server id: `api.ts` tags every id that comes in
 * as `<server>|<id>` and splits it again on the way out. Everything above the
 * API (tree, selection, activity, menus) keeps working on plain strings.
 */
import type { Tree } from "./api";

export const SEP = "|";

/** Link state of a connected server, or the ping result of a found one. */
export interface ServerStatus { state: "connecting" | "online" | "offline" | ""; version?: string | null; protocol?: number | null; error?: string | null }
/** One row of the SERVERS list (Rust `servers::ServerView`). */
export interface ServerView {
  id: string;
  name: string;
  kind: "herdr" | "deck-server";
  address: string;
  connected: boolean;
  manual: boolean;
  found: boolean;
  new: boolean;
  /** On another machine (an `ssh://` address). */
  remote?: boolean;
  status: ServerStatus;
}
export interface ServersView { servers: ServerView[]; scanned: boolean }

export const tagId = (server: string, id: string) => `${server}${SEP}${id}`;

/** `<server>|<id>` → parts; an untagged id belongs to no server (`""`). */
export function splitId(key: string): { server: string; id: string } {
  const i = key.lastIndexOf(SEP);
  return i < 0 ? { server: "", id: key } : { server: key.slice(0, i), id: key.slice(i + 1) };
}
export const serverOf = (key: string) => splitId(key).server;
export const localId = (key: string) => splitId(key).id;

/** Fields whose string value is a server-scoped id. */
const ID_KEYS = new Set(["workspace_id", "tab_id", "pane_id", "terminal_id", "active_tab_id", "open_workspace_id", "source_workspace_id", "target_pane_id"]);

/** Pure. A deep copy of `value` with every id field tagged with `server`. */
export function tagDeep<T>(server: string, value: T): T {
  if (!server) return value; // the single-server Deck: ids stay as the server sent them
  const walk = (v: unknown): unknown => {
    if (Array.isArray(v)) return v.map(walk);
    if (v === null || typeof v !== "object") return v;
    const out: Record<string, unknown> = {};
    for (const [k, x] of Object.entries(v as Record<string, unknown>)) {
      out[k] = ID_KEYS.has(k) && typeof x === "string" && x !== "" ? tagId(server, x) : walk(x);
    }
    return out;
  };
  return walk(value) as T;
}

/** Pure. One tree out of each server's (tagged) tree, in server order. */
export function mergeTrees(parts: { server: string; tree: Tree }[]): Tree {
  const out: Tree = { workspaces: [], tabs: [], panes: [], agents: [] };
  for (const { server, tree } of parts) {
    const t = tagDeep(server, tree);
    out.workspaces.push(...t.workspaces);
    out.tabs.push(...t.tabs);
    out.panes.push(...t.panes);
    out.agents.push(...t.agents);
  }
  return out;
}

/** Pure. Keep only one server's part of a merged tree (`null` = all). */
export function filterTree(tree: Tree | undefined, server: string | null): Tree | undefined {
  if (!tree || server === null) return tree;
  const mine = <X extends { workspace_id: string }>(xs: X[]) => xs.filter((x) => serverOf(x.workspace_id) === server);
  return { workspaces: mine(tree.workspaces), tabs: mine(tree.tabs), panes: mine(tree.panes), agents: mine(tree.agents) };
}

/**
 * Pure. `next` plus the last known part of each server in `down` (from
 * `prev`), so a server that stops answering keeps its tree, shown dimmed.
 */
export function keepDown(next: Tree, prev: Tree | undefined, down: string[]): Tree {
  if (!prev || !down.length) return next;
  const out: Tree = { workspaces: [...next.workspaces], tabs: [...next.tabs], panes: [...next.panes], agents: [...next.agents] };
  for (const id of down) {
    const t = filterTree(prev, id)!;
    out.workspaces.push(...t.workspaces); out.tabs.push(...t.tabs); out.panes.push(...t.panes); out.agents.push(...t.agents);
  }
  return out;
}

/** Pure. A stored id from before multi-server Deck (no `|`) moves to `server`. */
export function migrateKey(key: string, server: string | null): string {
  return key.includes(SEP) || !server ? key : tagId(server, key);
}

/** The servers the deck tab can show: connected ones, in preference order. */
export const connectedServers = (v: ServersView | undefined) => (v?.servers ?? []).filter((s) => s.connected);
/** Found, not connected: offered with CONNECT. */
export const offeredServers = (v: ServersView | undefined) => (v?.servers ?? []).filter((s) => !s.connected && s.found);

/** A server's dot: online, on the way, or down. */
export const dotOf = (s: ServerView) =>
  s.status.state === "online" ? "ok" : s.status.state === "offline" ? "bad" : s.connected ? "wait" : "off";


/** What to do about a server's error, in words (the raw error is the row's title). */
export function hintOf(error: string | null | undefined): string | null {
  if (!error) return null;
  if (/os error 5|access is denied|permission denied|runs elevated/i.test(error))
    return "The server runs as administrator. Start it with --allow-unelevated-clients or set [server] allow_unelevated_clients.";
  if (/not reachable|os error 2|cannot find|no such file|is `herdr server` running/i.test(error))
    return "Nothing answers at this address. Start the server, or check the path.";
  if (/^ssh |ssh bridge|publickey/i.test(error)) return "SSH could not reach the machine. Check that `ssh user@host` works without a password prompt (key or agent).";
  if (/timed out/i.test(error)) return "The server did not answer in time. It may be busy or hung.";
  return null;
}


/** The single-server Deck as a server list: one server with no id (ids stay untagged). */
export const LEGACY_SERVERS: ServersView = {
  servers: [{ id: "", name: "herdr", kind: "herdr", address: "", connected: true, manual: false, found: true, new: false, status: { state: "online" } }],
  scanned: true,
};
