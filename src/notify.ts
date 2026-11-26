/**
 * PREFERENCES › NOTIFICATIONS, as Herdr does them: when an agent finishes its
 * turn ("done") or needs the user ("request": a question, a permission
 * prompt), a Windows notification and/or a sound. Only for a session that is
 * not the one in view, or when the Deck window is not focused. Every
 * connected server's sessions count. The notification itself cannot carry a
 * click handler, so the session it was about is opened when Deck gets the
 * focus back shortly after it (clicking the toast does exactly that).
 */
import type { AgentStatus } from "./api";

export interface NotifySettings { done: boolean; request: boolean; sound: boolean }
export type NotifyKind = "done" | "request";

const KEY = "deck.notify";
export const NOTIFY_DEFAULTS: NotifySettings = { done: true, request: true, sound: true };

export function loadNotify(): NotifySettings {
  try {
    const v = JSON.parse(localStorage.getItem(KEY) ?? "{}") as Partial<NotifySettings>;
    return { ...NOTIFY_DEFAULTS, ...Object.fromEntries(Object.entries(v).filter(([, x]) => typeof x === "boolean")) };
  } catch { return { ...NOTIFY_DEFAULTS }; }
}

export function saveNotify(s: NotifySettings) {
  try { localStorage.setItem(KEY, JSON.stringify(s)); } catch { /* storage unavailable */ }
}

/**
 * Pure. What changed between two looks at the agents' statuses (pane → status):
 * working → idle/done is "done"; anything → blocked is "request". A pane seen
 * for the first time says nothing (a server just connected, a new session).
 */
export function transitions(prev: ReadonlyMap<string, AgentStatus>, now: ReadonlyMap<string, AgentStatus>): { pane: string; kind: NotifyKind }[] {
  const out: { pane: string; kind: NotifyKind }[] = [];
  for (const [pane, status] of now) {
    const before = prev.get(pane);
    if (before === undefined || before === status) continue;
    if (status === "blocked") out.push({ pane, kind: "request" });
    else if (before === "working" && (status === "idle" || status === "done")) out.push({ pane, kind: "done" });
  }
  return out;
}

/** Pure. Whether to tell the user: the kind is on, and they are not already looking at that session. */
export function shouldNotify(s: NotifySettings, kind: NotifyKind, pane: string, view: { focused: boolean; selected: string | null; deckView: boolean }): boolean {
  if (!s[kind]) return false;
  return !view.focused || !view.deckView || view.selected !== pane;
}

/** Pure. The notification's words. */
export function notifyText(kind: NotifyKind, where: string): { title: string; body: string } {
  return kind === "request"
    ? { title: "Claude needs you", body: `${where} — waiting for an answer` }
    : { title: "Claude is done", body: `${where} — finished its turn` };
}
