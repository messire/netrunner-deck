import { describe, expect, it } from "vitest";
import { commands, hotkeyOf, nextBlockedPane } from "./actions";
import type { Tree } from "./api";

const key = (k: string, mods: Partial<{ ctrlKey: boolean; altKey: boolean; metaKey: boolean; shiftKey: boolean }> = {}) =>
  ({ key: k, ctrlKey: false, altKey: false, metaKey: false, shiftKey: false, ...mods });

const tree: Tree = {
  workspaces: [
    { workspace_id: "w1", number: 1, label: "Elly2", focused: true, pane_count: 2, tab_count: 2, active_tab_id: "t1", agent_status: "working" },
    { workspace_id: "w2", number: 2, label: "SRE", focused: false, pane_count: 1, tab_count: 1, active_tab_id: "t3", agent_status: "blocked" },
  ],
  tabs: [
    { tab_id: "t1", workspace_id: "w1", number: 1, label: "main", focused: true, pane_count: 1, agent_status: "working" },
    { tab_id: "t2", workspace_id: "w1", number: 2, label: "tests", focused: false, pane_count: 1, agent_status: "blocked" },
    { tab_id: "t3", workspace_id: "w2", number: 1, label: "ops", focused: true, pane_count: 1, agent_status: "blocked" },
  ],
  panes: [
    { pane_id: "p1", terminal_id: "x1", workspace_id: "w1", tab_id: "t1", focused: true, agent: "claude", agent_status: "working" },
    { pane_id: "p2", terminal_id: "x2", workspace_id: "w1", tab_id: "t2", focused: false, agent: "claude", agent_status: "blocked" },
    { pane_id: "p3", terminal_id: "x3", workspace_id: "w2", tab_id: "t3", focused: true, agent: "claude", agent_status: "blocked" },
  ],
  agents: [
    { terminal_id: "x1", agent: "claude", agent_status: "working", workspace_id: "w1", tab_id: "t1", pane_id: "p1", focused: true },
    { terminal_id: "x2", agent: "claude", agent_status: "blocked", workspace_id: "w1", tab_id: "t2", pane_id: "p2", focused: false },
    { terminal_id: "x3", agent: "claude", agent_status: "blocked", workspace_id: "w2", tab_id: "t3", pane_id: "p3", focused: true },
  ],
};

describe("hotkeyOf", () => {
  it("claims F1, F2 and Ctrl+K regardless of overlays", () => {
    expect(hotkeyOf(key("F1"), false)).toBe("accounts");
    expect(hotkeyOf(key("F2"), false)).toBe("jump");
    expect(hotkeyOf(key("k", { ctrlKey: true }), false)).toBe("palette");
    expect(hotkeyOf(key("K", { ctrlKey: true, shiftKey: true }), true)).toBe("palette");
  });
  it("leaves Esc to the CLI unless an overlay is open", () => {
    expect(hotkeyOf(key("Escape"), false)).toBeNull();
    expect(hotkeyOf(key("Escape"), true)).toBe("close");
  });
  it("ignores modified function keys and ordinary keys", () => {
    expect(hotkeyOf(key("F1", { ctrlKey: true }), false)).toBeNull();
    expect(hotkeyOf(key("k"), false)).toBeNull();
    expect(hotkeyOf(key("k", { ctrlKey: true, altKey: true }), false)).toBeNull();
  });
});

describe("nextBlockedPane", () => {
  it("cycles through blocked agents after the current pane", () => {
    expect(nextBlockedPane(tree, "p1")).toBe("p2");
    expect(nextBlockedPane(tree, "p2")).toBe("p3");
    expect(nextBlockedPane(tree, "p3")).toBe("p2");
    expect(nextBlockedPane(tree, null)).toBe("p2");
  });
  it("is null without a tree or without blocked agents", () => {
    expect(nextBlockedPane(undefined, null)).toBeNull();
    expect(nextBlockedPane({ ...tree, agents: tree.agents.filter((a) => a.agent_status !== "blocked") }, "p1")).toBeNull();
  });
});

describe("commands", () => {
  const accounts = [
    { number: 1, email: "a@x", alias: "PLURIO", active: true },
    { number: 2, email: "b@x", active: false },
  ];
  it("offers context actions for the current pane's space and tab, then agents and accounts", () => {
    const c = commands(tree, accounts, "p2");
    expect(c.map((x) => x.id)).toEqual([
      "ws.new", "tab.new", "tab.close", "ws.close", "accounts.refresh",
      "agent:p1", "agent:p2", "agent:p3", "account:1", "account:2",
    ]);
    expect(c.find((x) => x.id === "tab.close")).toMatchObject({ danger: true, do: { kind: "action", action: "tab.close", id: "t2" } });
    expect(c.find((x) => x.id === "ws.new")!.do).toEqual({ kind: "action", action: "workspace.create", id: "w1" });
    expect(c.find((x) => x.id === "account:2")!.do).toEqual({ kind: "switch", account: 2 });
  });
  it("names agents like the sidebar: branch/label, plus the tab when a space has several agents", () => {
    const c = commands(tree, [], "p1");
    expect(c.find((x) => x.id === "agent:p1")!.label).toBe("◆ Elly2 · main");
    expect(c.find((x) => x.id === "agent:p3")!.label).toBe("◈ SRE");
  });
  it("without a selected pane only offers a new space and refresh before the lists", () => {
    const c = commands(tree, [], null);
    expect(c.slice(0, 2).map((x) => x.id)).toEqual(["ws.new", "accounts.refresh"]);
    expect(c[0].do).toEqual({ kind: "action", action: "workspace.create", id: undefined });
  });
});
