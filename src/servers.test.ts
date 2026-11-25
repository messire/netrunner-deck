import { describe, expect, it } from "vitest";
import type { Tree } from "./api";
import { buildTree } from "./tree";
import { connectedServers, hintOf, machineErrorOf, machineOfServer, machinesOf, validateHostForm, filterTree, keepDown, localId, mergeTrees, migrateKey, offeredServers, serverOf, splitId, tagDeep, tagId, type ServerView } from "./servers";

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
  const merged = mergeTrees([{ server: "herdr", tree: one("a", "atlas") }, { server: "deck-server/try", tree: one("b", "atlas") }]);

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
    expect(roots.map((r) => r.id)).toEqual(["repo:herdr|atlas", "repo:deck-server/try|atlas"]);
    // An untagged (single-server, test) tree keeps the old ids.
    expect(buildTree(one("a", "atlas"))[0].id).toBe("repo:atlas");
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

describe("ADD machine dialog", () => {
  const f = (user: string, host: string, port: string) => validateHostForm({ user, host, port });
  it("accepts user, host and a port, 22 by default", () => {
    expect(f(" dev ", " build-box.example.com ", "2222")).toEqual({ ok: true, user: "dev", host: "build-box.example.com", port: 2222 });
    expect(f("dev", "build-box.example.com", "")).toMatchObject({ ok: true, port: 22 });
    expect(f("dev", "[fe80::1]", "22")).toMatchObject({ ok: true, host: "fe80::1" });
    expect(f("dev", "192.0.2.7", "65535")).toMatchObject({ ok: true, port: 65535 });
  });
  it("names the field that is wrong", () => {
    expect(f("", "build-box.example.com", "22")).toMatchObject({ ok: false, field: "user" });
    expect(f("dev@x", "build-box.example.com", "22")).toMatchObject({ ok: false, field: "user" });
    expect(f("dev", "", "22")).toMatchObject({ ok: false, field: "host" });
    expect(f("dev", "dev@build-box.example.com", "22")).toMatchObject({ ok: false, field: "host" });
    expect(f("dev", "build-box.example.com:2222", "22")).toMatchObject({ ok: false, field: "host" });
    expect(f("dev", "-oProxyCommand=x", "22")).toMatchObject({ ok: false, field: "host" });
    for (const port of ["0", "65536", "22a", "-1", "2.5"]) expect(f("dev", "build-box.example.com", port)).toMatchObject({ ok: false, field: "port" });
  });
  it("says why a machine cannot be used in plain words", () => {
    expect(machineErrorOf("ssh dev@build-box.example.com:2222: no answer within 20s")).toMatch(/No answer/);
    expect(machineErrorOf("ssh dev@build-box.example.com: dev@build-box.example.com: Permission denied (publickey).")).toMatch(/refused the sign-in/);
    expect(machineErrorOf("bridge on dev@build-box.example.com: no deck-server with a bridge there")).toMatch(/deck-server is not installed/);
    expect(machineErrorOf("no Herdr or deck-server is running on dev@build-box.example.com")).toMatch(/no Herdr or deck-server runs/);
    expect(machineErrorOf("ssh: Could not resolve hostname nowhere.example.com")).toMatch(/not known/);
    expect(machineErrorOf("something odd")).toBe("something odd");
  });
});

describe("machinesOf", () => {
  const srv = (id: string, kind: "herdr" | "deck-server", state: "online" | "offline" | "connecting", machine?: string, connected = true): ServerView =>
    ({ id, name: id, kind, address: "", connected, manual: false, found: true, new: false, remote: !!machine, machine: machine ?? null, status: { state } });
  it("lists each machine once, asking its deck-server, remote ones included", () => {
    const servers = [
      srv("herdr", "herdr", "online"),
      srv("deck-server/try", "deck-server", "online"),
      srv("dev@box.example.com/herdr", "herdr", "online", "dev@box.example.com"),
      srv("dev@box.example.com/deck-server", "deck-server", "connecting", "dev@box.example.com"),
      srv("dev@down.example.com/deck-server", "deck-server", "offline", "dev@down.example.com"),
      srv("dev@off.example.com/deck-server", "deck-server", "online", "dev@off.example.com", false),
    ];
    const ms = machinesOf(servers);
    expect(ms.map((m) => [m.key, m.id])).toEqual([["local", "deck-server/try"], ["dev@box.example.com", "dev@box.example.com/deck-server"]]);
    expect(machineOfServer(servers, ms, "dev@box.example.com/herdr")?.id).toBe("dev@box.example.com/deck-server");
    expect(machineOfServer(servers, ms, "herdr")?.key).toBe("local");
    expect(machineOfServer(servers, ms, "nope")).toBeUndefined();
  });
});
