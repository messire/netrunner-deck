import { describe, expect, it } from "vitest";
import type { Tree } from "./api";
import { buildTree } from "./tree";
import { connectedServers, hintOf, filterTree, keepDown, localId, mergeTrees, migrateKey, offeredServers, serverOf, splitId, tagDeep, tagId, type ServerView } from "./servers";

const one = (label: string, repo?: string): Tree => ({
  workspaces: [{ workspace_id: "w1", number: 1, label, focused: true, pane_count: 1, tab_count: 1, active_tab_id: "w1:t1", agent_status: "idle",
    worktree: repo ? { repo_key: repo, repo_name: repo, is_linked_worktree: false } : null }],
  tabs: [{ tab_id: "w1:t1", workspace_id: "w1", number: 1, label: "1", focused: true, pane_count: 1, agent_status: "idle" }],
  panes: [{ pane_id: "w1:p1", terminal_id: "t1", workspace_id: "w1", tab_id: "w1:t1", focused: true, agent: "claude", agent_status: "idle" }],
  agents: [{ terminal_id: "t1", agent_status: "idle", workspace_id: "w1", tab_id: "w1:t1", pane_id: "w1:p1", focused: true }],
});

describe("server-scoped ids", () => {
  it("tags and splits, with the server keeping its own slashes", () => {
    const k = tagId("deck-server/try", "w1:p1");
    expect(k).toBe("deck-server/try|w1:p1");
    expect(splitId(k)).toEqual({ server: "deck-server/try", id: "w1:p1" });
    expect(serverOf(k)).toBe("deck-server/try");
    expect(localId(k)).toBe("w1:p1");
    expect(splitId("w1:p1")).toEqual({ server: "", id: "w1:p1" });
  });

  it("tagDeep tags id fields at any depth and nothing else", () => {
    const ev = { event: "pane_updated", pane: { pane_id: "w1:p1", workspace_id: "w1", label: "w1:p1", tokens: { ctx: "4" } }, data: { pane_id: "w1:p2", revision: 3 }, empty: { pane_id: "" } };
    const t = tagDeep("herdr", ev);
    expect(t.pane.pane_id).toBe("herdr|w1:p1");
    expect(t.pane.workspace_id).toBe("herdr|w1");
    expect(t.pane.label).toBe("w1:p1");
    expect(t.data).toEqual({ pane_id: "herdr|w1:p2", revision: 3 });
    expect(t.empty.pane_id).toBe("");
    expect(ev.pane.pane_id).toBe("w1:p1"); // input untouched
  });

  it("migrates ids remembered before multi-server Deck", () => {
    expect(migrateKey("w1:p1", "herdr")).toBe("herdr|w1:p1");
    expect(migrateKey("herdr|w1:p1", "other")).toBe("herdr|w1:p1");
    expect(migrateKey("w1:p1", null)).toBe("w1:p1");
  });
});

describe("merged trees", () => {
  const merged = mergeTrees([{ server: "herdr", tree: one("a", "elly") }, { server: "deck-server/try", tree: one("b", "elly") }]);

  it("keeps equal local ids of two servers apart", () => {
    expect(merged.panes.map((p) => p.pane_id)).toEqual(["herdr|w1:p1", "deck-server/try|w1:p1"]);
    expect(merged.agents.map((a) => a.pane_id)).toEqual(["herdr|w1:p1", "deck-server/try|w1:p1"]);
    expect(merged.workspaces[1].active_tab_id).toBe("deck-server/try|w1:t1");
  });

  it("filters one server back out; null = all", () => {
    const b = filterTree(merged, "deck-server/try")!;
    expect(b.workspaces.map((w) => w.label)).toEqual(["b"]);
    expect(b.panes).toHaveLength(1);
    expect(filterTree(merged, null)).toBe(merged);
    expect(filterTree(undefined, "herdr")).toBeUndefined();
  });

  it("buildTree keeps the same repo on two servers as two repos", () => {
    const roots = buildTree(merged);
    expect(roots.map((r) => r.id)).toEqual(["repo:herdr|elly", "repo:deck-server/try|elly"]);
    // An untagged (single-server, test) tree keeps the old ids.
    expect(buildTree(one("a", "elly"))[0].id).toBe("repo:elly");
  });

  it("keepDown keeps a failed server's last known part", () => {
    const now = mergeTrees([{ server: "herdr", tree: one("a2") }]);
    const kept = keepDown(now, merged, ["deck-server/try"]);
    expect(kept.workspaces.map((w) => `${serverOf(w.workspace_id)}:${w.label}`)).toEqual(["herdr:a2", "deck-server/try:b"]);
    expect(keepDown(now, undefined, ["x"])).toBe(now);
  });
});

describe("server lists", () => {
  const s = (id: string, connected: boolean, found = true): ServerView =>
    ({ id, name: id, kind: "herdr", address: id, connected, manual: false, found, new: !connected, status: { state: "online" } });
  it("splits connected from offered", () => {
    const v = { servers: [s("a", true), s("b", false), s("c", true, false), s("d", false, false)], scanned: true };
    expect(connectedServers(v).map((x) => x.id)).toEqual(["a", "c"]);
    expect(offeredServers(v).map((x) => x.id)).toEqual(["b"]);
    expect(connectedServers(undefined)).toEqual([]);
  });

  it("explains the common connect errors", () => {
    expect(hintOf("herdr server is not reachable at x: Access is denied. (os error 5)")).toMatch(/administrator/);
    expect(hintOf("herdr server is not reachable at x: The system cannot find the file specified. (os error 2)")).toMatch(/Nothing answers/);
    expect(hintOf("timed out waiting for herdr to answer ping")).toMatch(/in time/);
    expect(hintOf("something else")).toBeNull();
    expect(hintOf(null)).toBeNull();
  });
});
