/**
 * Live state of every headless Claude pane on screen or in the tree
 * (`agent.ts` folds; this keeps one stream per pane open and the result in
 * a signal). `syncAgents` is called with the panes that run
 * `deck-server agent`; streams of panes that went away are closed.
 */
import { createSignal } from "solid-js";
import { api } from "./api";
import { foldAgentLine, type AgentLive } from "./agent";

const [live, setLive] = createSignal<ReadonlyMap<string, AgentLive>>(new Map());
const watched = new Set<string>();
let listening: Promise<unknown> | null = null;

function listenOnce() {
  listening ??= api.onAgent((pane, line) => {
    if (!watched.has(pane)) return;
    setLive((m) => {
      const next = new Map(m);
      next.set(pane, foldAgentLine(m.get(pane), line));
      return next;
    });
  });
  return listening;
}

/** The live state of a headless pane, once its stream said anything. */
export const agentOf = (pane: string): AgentLive | undefined => live().get(pane);

/** Keep exactly these panes streamed. */
export async function syncAgents(panes: string[]) {
  await listenOnce();
  const want = new Set(panes);
  for (const p of want) {
    if (watched.has(p)) continue;
    watched.add(p);
    api.agentWatch(p).catch(() => watched.delete(p));
  }
  for (const p of [...watched]) {
    if (want.has(p)) continue;
    watched.delete(p);
    api.agentUnwatch(p).catch(() => {});
    setLive((m) => { if (!m.has(p)) return m; const next = new Map(m); next.delete(p); return next; });
  }
}
