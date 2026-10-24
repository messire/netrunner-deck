/**
 * Routing of Herdr `events.subscribe` events (docs/architecture.md F1–F3).
 *
 * Every event is metadata about the tree (workspaces, tabs, panes, agents).
 * There is no output event: `pane_updated` carries `pane: PaneInfo` (F2) and
 * `pane_output_changed` exists in the schema but is never emitted (F3), so no
 * event may drive a terminal refresh — the pane view polls (TerminalView).
 * When the fork starts emitting `pane_output_changed` (M1.5) this is the place
 * to add a `"tick"` route gated on `currentPane`.
 */
export type EventRoute = "tree" | "ignore";

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

/** Pure. `currentPane` is unused until a per-pane route exists (see above). */
export function routeEvent(ev: unknown, _currentPane: string | null): EventRoute {
  if (typeof ev !== "object" || ev === null || Array.isArray(ev)) return "ignore";
  const raw = (ev as { event?: unknown }).event;
  if (typeof raw !== "string" || raw === "") return "ignore";
  // The wire form is snake_case; subscriptions use dotted names (F1) — be
  // tolerant of the dotted spelling should it ever be echoed back.
  const name = raw.replace(/\./g, "_");
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
