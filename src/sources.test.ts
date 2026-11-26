import { describe, expect, it } from "vitest";
import type { ServerView } from "./servers";
import { belongsTo, homeOf, kindLabel, shownServers, sourceError, sourcesOf } from "./sources";

const srv = (id: string, extra: Partial<ServerView> = {}): ServerView => ({
  id, name: id, kind: "deck-server", address: "", connected: true, manual: false, found: true, new: false, status: { state: "online" }, ...extra,
});

describe("sources", () => {
  const local = srv("deck-server/try");
  const herdr = srv("herdr", { kind: "herdr" });
  const remote = srv("u@box/deck-server", { remote: true, machine: "u@box" });

  it("shows the picked server, else every connected one", () => {
    expect(shownServers([local, remote], "u@box/deck-server")).toEqual([remote]);
    expect(shownServers([local, remote], "all")).toEqual([local, remote]);
    expect(shownServers([local, remote], "gone")).toEqual([local, remote]);
  });

  it("builds one source per server, offline ones with their reason", () => {
    const down = srv("x", { status: { state: "offline", error: "refused" } });
    const s = sourcesOf([local, down], { [local.id]: { data: [1] } });
    expect(s.map((x) => [x.id, x.data, x.error])).toEqual([[local.id, [1], null], ["x", undefined, "refused"]]);
  });

  it("tells an older deck-server from other failures", () => {
    expect(sourceError("the server refused deck.archive.scan: not a deck machine method")).toMatch(/older than this Deck/);
    expect(sourceError("ssh: timeout")).toBe("ssh: timeout");
  });

  it("gives untagged log entries to one server on this computer, a deck-server first", () => {
    const home = homeOf([remote, herdr, local]);
    expect(home).toBe(local.id);
    expect(homeOf([remote, herdr])).toBe(herdr.id);
    expect(belongsTo({ server: null }, local, home)).toBe(true);
    expect(belongsTo({}, herdr, home)).toBe(false);
    expect(belongsTo({}, remote, home)).toBe(false);
    expect(belongsTo({ server: remote.id }, remote, home)).toBe(true);
    expect(belongsTo({ server: remote.id }, local, home)).toBe(false);
  });

  it("names the kind of server", () => {
    expect(kindLabel(local.kind)).toBe("DECK");
    expect(kindLabel(herdr.kind)).toBe("HERDR");
  });
});
