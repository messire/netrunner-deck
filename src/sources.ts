/**
 * HARNESS, UPLINK, ARCHIVE and QUICKHACKS follow the server picked on the
 * deck tab: what they show comes from that server's machine (`deck.harness.*`,
 * `deck.uplink.*`, `deck.archive.*` through its bridge). With ALL, every
 * connected server is a source of its own.
 */
import type { ServerView } from "./servers";

/** What one server contributes to a view. `data` is undefined while loading. */
export interface Source<T> { id: string; name: string; kind: ServerView["kind"]; remote: boolean; data: T | undefined; error: string | null }

export const kindLabel = (kind: ServerView["kind"]) => (kind === "deck-server" ? "DECK" : "HERDR");

/** Pure. The servers a view shows: the picked one, else every connected one (in order). */
export function shownServers(connected: ServerView[], scope: string): ServerView[] {
  if (scope === "all") return connected;
  const s = connected.find((x) => x.id === scope);
  return s ? [s] : connected;
}

/** Pure. One source per shown server, from what was fetched per server id. */
export function sourcesOf<T>(servers: ServerView[], fetched: Record<string, { data?: T; error?: string | null }>): Source<T>[] {
  return servers.map((s) => ({
    id: s.id, name: s.name, kind: s.kind, remote: !!s.remote,
    data: fetched[s.id]?.data,
    error: fetched[s.id]?.error ?? (s.status.state === "offline" ? s.status.error ?? "offline" : null),
  }));
}

/** Pure. A machine error in plain words: an older deck-server bridge does not know these methods. */
export function sourceError(error: unknown): string {
  const e = String(error);
  if (/not a deck machine method|unknown method deck\.(harness|uplink|archive)|unknown machine method/i.test(e))
    return "deck-server on that machine is older than this Deck: update it to see this here";
  return e;
}

/**
 * Pure. Whether an entry Deck logged (a pull, a pinned hack) belongs to
 * `server`: its own tag; an untagged one (from before entries had a
 * server) belongs to `home`, the first connected server on this computer.
 */
export function belongsTo(entry: { server?: string | null }, server: ServerView, home: string | undefined): boolean {
  return entry.server ? entry.server === server.id : server.id === home;
}

/** Pure. Where untagged entries live: the first connected deck-server on this computer, else its first server. */
export const homeOf = (connected: ServerView[]) => {
  const local = connected.filter((s) => !s.remote);
  return (local.find((s) => s.kind === "deck-server") ?? local[0])?.id;
};
