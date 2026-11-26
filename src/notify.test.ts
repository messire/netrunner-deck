import { describe, expect, it } from "vitest";
import type { AgentStatus } from "./api";
import { NOTIFY_DEFAULTS, notifyText, shouldNotify, transitions } from "./notify";

const m = (o: Record<string, AgentStatus>) => new Map(Object.entries(o));

describe("notifications", () => {
  it("turns status changes into done and request", () => {
    const got = transitions(m({ a: "working", b: "working", c: "idle", d: "blocked" }), m({ a: "idle", b: "blocked", c: "idle", d: "working", e: "blocked" }));
    expect(got).toEqual([{ pane: "a", kind: "done" }, { pane: "b", kind: "request" }]);
  });

  it("stays quiet for the session in view of a focused Deck", () => {
    const view = { focused: true, selected: "a", deckView: true };
    expect(shouldNotify(NOTIFY_DEFAULTS, "done", "a", view)).toBe(false);
    expect(shouldNotify(NOTIFY_DEFAULTS, "done", "b", view)).toBe(true);
    expect(shouldNotify(NOTIFY_DEFAULTS, "done", "a", { ...view, focused: false })).toBe(true);
    expect(shouldNotify(NOTIFY_DEFAULTS, "done", "a", { ...view, deckView: false })).toBe(true);
    expect(shouldNotify({ ...NOTIFY_DEFAULTS, request: false }, "request", "b", view)).toBe(false);
  });

  it("names the session", () => {
    expect(notifyText("request", "atlas · main").body).toContain("atlas · main");
  });
});
