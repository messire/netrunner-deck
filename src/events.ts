/**
 * Routing of Herdr `events.subscribe` events (docs/architecture.md F1–F3).
 *
 * Almost every event is metadata about the tree (workspaces, tabs, panes,
 * agents): `pane_updated` carries `pane: PaneInfo` (F2), not output. The one
 * output event is the fork's `pane_output_changed` (M1.5): coalesced per pane
 * on the server, with the pane's content `revision`. It becomes a `"tick"`
 * only for the pane on screen; other panes' output is irrelevant to the view
 * (attention is shown on the session map from `pane_updated`). Upstream Herdr
 * never sends it, so the pane view keeps a slow poll as fallback.
 */
export type EventRoute = "tree" | "tick" | "ignore";

const TREE_PREFIXES = ["workspace_", "worktree_", "tab_"];
const TREE_EVENTS = new Set([
  "pane_created",
  "pane_closed",
  "pane_updated",
  "pane_focused",
  "pane_moved",
  "pane_exited",
  "pane_agent_detected",
  "pane_agent_status_changed",
  "layout_updated",
]);

/** Pure. `currentPane` gates the output tick to the pane on screen. */
export function routeEvent(ev: unknown, currentPane: string | null): EventRoute {
  if (typeof ev !== "object" || ev === null || Array.isArray(ev)) return "ignore";
  const raw = (ev as { event?: unknown }).event;
  if (typeof raw !== "string" || raw === "") return "ignore";
  // The wire form is snake_case; subscriptions use dotted names (F1) — be
  // tolerant of the dotted spelling should it ever be echoed back.
  const name = raw.replace(/\./g, "_");
  if (name === "pane_output_changed") {
    const pane = (ev as { pane_id?: unknown }).pane_id;
    return currentPane !== null && pane === currentPane ? "tick" : "ignore";
  }
  if (TREE_EVENTS.has(name)) return "tree";
  if (TREE_PREFIXES.some((p) => name.startsWith(p))) return "tree";
  return "ignore";
}

/**
 * Trailing-edge debounce: a burst of calls becomes one call with the last
 * arguments, `ms` after the burst ends. `cancel()` drops a pending call.
 */
export function debounceTrailing<T extends (...a: any[]) => void>(fn: T, ms: number): T & { cancel(): void } {
  let timer: ReturnType<typeof setTimeout> | undefined;
  const debounced = ((...args: Parameters<T>) => {
    if (timer !== undefined) clearTimeout(timer);
    timer = setTimeout(() => {
      timer = undefined;
      fn(...args);
    }, ms);
  }) as T & { cancel(): void };
  debounced.cancel = () => {
    if (timer !== undefined) clearTimeout(timer);
    timer = undefined;
  };
  return debounced;
}
