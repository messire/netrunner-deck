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
  /** That machine, `user@host[:port]`: what REMOVE takes. */
  machine?: string | null;
  status: ServerStatus;
}
/** A saved machine the last scan could not reach: one grey row. */
export interface MachineDown { machine: string; error: string }
export interface ServersView { servers: ServerView[]; scanned: boolean; unreachable?: MachineDown[] }
/** One server CHECK found on a machine (Rust `servers::Probed`). */
export interface Probed { id: string; name: string; kind: "herdr" | "deck-server"; socket: string; version?: string | null; protocol?: number | null; preferred: boolean; connected: boolean }

/** The ADD dialog's fields as typed. */
export interface HostForm { user: string; host: string; port: string }
/** Pure. Check the ADD dialog's fields; the error names the field. */
export function validateHostForm(f: HostForm): { ok: true; user: string; host: string; port: number } | { ok: false; field: keyof HostForm; error: string } {
  const user = f.user.trim(), host = f.host.trim().replace(/^\[(.*)\]$/, "$1"), port = f.port.trim();
  if (!user) return { ok: false, field: "user", error: "Enter the user to sign in as." };
  if (/[\s@/\\:]/.test(user) || user.startsWith("-")) return { ok: false, field: "user", error: "A user name has no spaces, @, : or slashes." };
  if (!host) return { ok: false, field: "host", error: "Enter the machine's host name or address." };
  if (/[\s@/\\]/.test(host) || host.startsWith("-")) return { ok: false, field: "host", error: "Only the host name or address goes here (no user, path or spaces)." };
  if (host.includes(":") && !/^[0-9a-f:.]+$/i.test(host)) return { ok: false, field: "host", error: "The port has its own field." };
  if (!/^\d+$/.test(port || "22")) return { ok: false, field: "port", error: "The port is a number, 1 to 65535." };
  const n = Number(port || "22");
  if (n < 1 || n > 65535) return { ok: false, field: "port", error: "The port is a number, 1 to 65535." };
  return { ok: true, user, host, port: n };
}

/** Pure. Why a machine cannot be used, in plain words (the raw error stays the title). */
export function machineErrorOf(error: string | null | undefined): string {
  const e = error ?? "";
  if (/permission denied|publickey|authentication|too many authentication/i.test(e))
    return "The machine refused the sign-in. Deck signs in with your SSH key or agent and cannot type a password.";
  if (/no answer within|timed out|connection timed out|operation timed out/i.test(e))
    return "No answer from the machine in time: it is off, the address or port is wrong, or a firewall blocks it.";
  if (/could not resolve|name or service not known|no such host|nodename nor servname/i.test(e))
    return "That host name is not known. Check the address.";
  if (/connection refused/i.test(e) && /^ssh |port \d+/i.test(e))
    return "Nothing accepts SSH at that address and port.";
  if (/host key verification failed|remote host identification has changed/i.test(e))
    return "The machine's SSH host key changed since the last visit, so Deck does not trust it.";
  if (/no deck-server with a bridge|deck-server.*not found|command not found/i.test(e))
    return "deck-server is not installed on that machine (or is too old to have a bridge). Every server there is reached through it.";
  if (/no herdr or deck-server is running/i.test(e)) return "The machine answers, but no Herdr or deck-server runs there.";
  if (/cannot start ssh/i.test(e)) return "Deck could not start ssh on this computer (OpenSSH client missing).";
  return e || "The machine cannot be reached.";
}

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
  if (/bridge on |^ssh |publickey/i.test(error)) return "SSH could not reach the machine. Check that `ssh user@host` works without a password prompt (key or agent).";
  if (/timed out/i.test(error)) return "The server did not answer in time. It may be busy or hung.";
  return null;
}


/** The single-server Deck as a server list: one server with no id (ids stay untagged). */
export const LEGACY_SERVERS: ServersView = {
  servers: [{ id: "", name: "herdr", kind: "herdr", address: "", connected: true, manual: false, found: true, new: false, status: { state: "online" } }],
  scanned: true,
};

/** One machine Deck reaches, for the per-machine panels (SECRETS): the server whose connection asks it. */
export interface MachineView { key: string; id: string; name: string }

/**
 * Pure. Every machine with a connected server, once: `local` for this PC,
 * else the server's `user@host[:port]`. The server asked is a deck-server
 * if the machine has one, then an online one; any server works, since
 * machine work is answered by Deck's bridge there.
 */
export function machinesOf(servers: ServerView[]): MachineView[] {
  const rank = (s: ServerView) => (s.kind === "deck-server" ? 0 : 2) + (s.status.state === "online" ? 0 : 1);
  const best = new Map<string, ServerView>();
  for (const s of servers) {
    if (!s.connected || s.status.state === "offline") continue;
    const key = s.machine ?? (s.remote ? s.id : "local");
    const had = best.get(key);
    if (!had || rank(s) < rank(had)) best.set(key, s);
  }
  return [...best].map(([key, s]) => ({ key, id: s.id, name: s.name }));
}

/** Pure. The machine `server` is on, among `machines` built from `servers`. */
export function machineOfServer(servers: ServerView[], machines: MachineView[], server: string | null | undefined): MachineView | undefined {
  const s = servers.find((x) => x.id === server);
  if (!s) return undefined;
  const key = s.machine ?? (s.remote ? s.id : "local");
  return machines.find((m) => m.key === key);
}
