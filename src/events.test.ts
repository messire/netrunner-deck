import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { debounceTrailing, routeEvent } from "./events";

// Fixture payloads follow the real wire shape (docs/architecture.md F1/F2):
// `event` is snake_case and the rest of the object is the variant payload.
const PANE_INFO = {
  pane_id: "pane-1", terminal_id: "t-1", workspace_id: "ws-1", tab_id: "tab-1",
  focused: true, title: "claude", agent: "claude", agent_status: "working", tokens: { ctx: "42" },
};

describe("routeEvent", () => {
  it("pane_updated is metadata (F2): refetch the tree, never a terminal tick", () => {
    // Same event, whether or not the pane is the one on screen.
    expect(routeEvent({ event: "pane_updated", pane: PANE_INFO }, "pane-1")).toBe("tree");
    expect(routeEvent({ event: "pane_updated", pane: PANE_INFO }, "pane-2")).toBe("tree");
    expect(routeEvent({ event: "pane_updated", pane: PANE_INFO }, null)).toBe("tree");
  });

  it.each([
    ["workspace_created", { workspace: { workspace_id: "ws-2", number: 2 } }],
    ["workspace_updated", { workspace_id: "ws-1" }],
    ["workspace_metadata_updated", { workspace_id: "ws-1" }],
    ["workspace_closed", { workspace_id: "ws-1" }],
    ["workspace_renamed", { workspace_id: "ws-1", label: "api" }],
    ["workspace_moved", { workspace_id: "ws-1" }],
    ["workspace_reordered", {}],
    ["workspace_focused", { workspace_id: "ws-1" }],
    ["worktree_created", { worktree: { repo_key: "r", repo_name: "deck" } }],
    ["worktree_opened", {}],
    ["worktree_removed", {}],
    ["tab_created", { tab: { tab_id: "tab-2", workspace_id: "ws-1" } }],
    ["tab_closed", { tab_id: "tab-2", workspace_id: "ws-1" }],
    ["tab_focused", { tab_id: "tab-1", workspace_id: "ws-1" }],
    ["tab_renamed", { tab_id: "tab-1", workspace_id: "ws-1", label: "x" }],
    ["tab_moved", { tab_id: "tab-1", workspace_id: "ws-1" }],
    ["pane_created", { pane: PANE_INFO }],
    ["pane_closed", { pane_id: "pane-1", workspace_id: "ws-1" }],
    ["pane_focused", { pane_id: "pane-1", workspace_id: "ws-1" }],
    ["pane_moved", { previous_pane_id: "pane-1", previous_workspace_id: "ws-1", previous_tab_id: "tab-1", pane: PANE_INFO }],
    ["pane_exited", { pane_id: "pane-1", workspace_id: "ws-1" }],
    ["pane_agent_detected", { pane_id: "pane-1", workspace_id: "ws-1", agent: "claude" }],
    ["pane_agent_status_changed", { pane_id: "pane-1", workspace_id: "ws-1", agent_status: "blocked" }],
    ["layout_updated", { workspace_id: "ws-1" }],
  ])("%s is structural → tree", (event, payload) => {
    expect(routeEvent({ event, ...payload }, "pane-1")).toBe("tree");
  });

  describe("pane_output_changed (fork M1.5) ticks only the pane on screen", () => {
    const out = (pane_id: string) => ({ event: "pane_output_changed", pane_id, workspace_id: "ws-1", revision: 7 });
    it("same pane → tick", () => {
      expect(routeEvent(out("pane-1"), "pane-1")).toBe("tick");
    });
    it("other pane → ignore (never a tree refetch: it carries no metadata)", () => {
      expect(routeEvent(out("pane-2"), "pane-1")).toBe("ignore");
    });
    it("no pane on screen → ignore", () => {
      expect(routeEvent(out("pane-1"), null)).toBe("ignore");
    });
    it("malformed payload without pane_id → ignore", () => {
      expect(routeEvent({ event: "pane_output_changed", revision: 7 }, "pane-1")).toBe("ignore");
      expect(routeEvent({ event: "pane_output_changed", pane_id: 3 }, "3")).toBe("ignore");
    });
    it("dotted spelling is tolerated", () => {
      expect(routeEvent({ event: "pane.output_changed", pane_id: "pane-1" }, "pane-1")).toBe("tick");
    });
  });

  it("per-pane-only events that cannot arrive on the global subscription are ignored", () => {
    expect(routeEvent({ event: "pane_scroll_changed", pane_id: "pane-1" }, "pane-1")).toBe("ignore");
    expect(routeEvent({ event: "pane_output_matched", pane_id: "pane-1", match: { type: "substring", value: "x" } }, "pane-1")).toBe("ignore");
  });

  it("unknown event names are ignored", () => {
    expect(routeEvent({ event: "pane_teleported" }, "pane-1")).toBe("ignore");
    expect(routeEvent({ event: "panes_updated" }, "pane-1")).toBe("ignore");
    expect(routeEvent({ event: "" }, null)).toBe("ignore");
    expect(routeEvent({ event: "ping" }, null)).toBe("ignore");
  });

  it("workspace_* / worktree_* / tab_* families are routed by prefix (new fork events refetch too)", () => {
    expect(routeEvent({ event: "tab_updated" }, null)).toBe("tree");
    expect(routeEvent({ event: "workspace_whatever" }, null)).toBe("tree");
  });

  it("accepts the dotted subscription spelling as well (F1)", () => {
    expect(routeEvent({ event: "pane.updated", pane: PANE_INFO }, "pane-1")).toBe("tree");
  });

  it("malformed input is ignored, never throws", () => {
    for (const bad of [null, undefined, 42, "pane_updated", [], {}, { type: "pane.updated" }, { event: 5 }, { event: null }, { event: { nested: true } }]) {
      expect(routeEvent(bad, "pane-1")).toBe("ignore");
    }
  });

  it("is pure: same input, same answer, input not mutated", () => {
    const ev = { event: "tab_created", tab: { tab_id: "tab-9" } };
    const snapshot = JSON.stringify(ev);
    expect(routeEvent(ev, null)).toBe(routeEvent(ev, null));
    expect(JSON.stringify(ev)).toBe(snapshot);
  });
});

describe("debounceTrailing", () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  it("a burst collapses into one trailing call with the last arguments", () => {
    const fn = vi.fn<(n: number) => void>();
    const d = debounceTrailing(fn, 200);
    // Ten events 50 ms apart (each one inside the previous quiet window).
    d(1);
    for (let i = 2; i <= 10; i++) { vi.advanceTimersByTime(50); d(i); }
    expect(fn).not.toHaveBeenCalled();
    vi.advanceTimersByTime(199);
    expect(fn).not.toHaveBeenCalled();
    vi.advanceTimersByTime(1);
    expect(fn).toHaveBeenCalledTimes(1);
    expect(fn).toHaveBeenLastCalledWith(10);
  });

  it("does not fire before the quiet period and fires again for a later burst", () => {
    const fn = vi.fn();
    const d = debounceTrailing(fn, 200);
    d();
    vi.advanceTimersByTime(200);
    expect(fn).toHaveBeenCalledTimes(1);
    d(); d();
    vi.advanceTimersByTime(200);
    expect(fn).toHaveBeenCalledTimes(2);
  });

  it("cancel() drops the pending call; the debouncer stays usable", () => {
    const fn = vi.fn();
    const d = debounceTrailing(fn, 200);
    d();
    d.cancel();
    vi.advanceTimersByTime(1000);
    expect(fn).not.toHaveBeenCalled();
    d();
    vi.advanceTimersByTime(200);
    expect(fn).toHaveBeenCalledTimes(1);
  });

  it("cancel() on an idle debouncer is a no-op", () => {
    const d = debounceTrailing(() => {}, 200);
    expect(() => { d.cancel(); d.cancel(); }).not.toThrow();
  });
});
