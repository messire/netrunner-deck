import { describe, expect, it } from "vitest";
import type { HarnessRepo } from "./api";
import { BUILTINS, slashCurrent, slashItems, slashMatches, slashQuery } from "./slash";

const item = (kind: string, name: string, description = "", argument_hint?: string) => ({ kind, name, path: `/${name}.md`, size: 1, mtime_ms: 0, local: false, tags: [], description, argument_hint });
const repos: HarnessRepo[] = [
  { key: "personal", name: "Personal", root: "C:/Users/me/.claude", personal: true, exists: true, items: [item("skill", "copy-profile", "user skill"), item("command", "ir-feedback", "user copy", "<period>")] },
  { key: "elly2", name: "Elly2", root: "\\\\?\\C:\\Workspace\\repos\\Elly2", personal: false, exists: true, items: [item("skill", "ir-feedback", "project skill"), item("command", "git/sync", "sync"), item("agent", "reviewer")] },
  { key: "elly2-wt", name: "Elly2-wt", root: "C:/Workspace/repos", personal: false, exists: true, items: [item("skill", "outer")] },
];

describe("slashItems", () => {
  it("deepest containing repo first, then personal, then built-ins; project names shadow personal ones", () => {
    const items = slashItems(repos, "C:\\Workspace\\repos\\Elly2\\src");
    expect(items.slice(0, 3).map((i) => [i.name, i.kind, i.source])).toEqual([["ir-feedback", "skill", "project"], ["sync", "command", "project:git"], ["copy-profile", "skill", "user"]]);
    expect(items.filter((i) => i.name === "ir-feedback")).toHaveLength(1);
    expect(items.some((i) => i.name === "help" && i.kind === "builtin")).toBe(true);
    expect(items.some((i) => i.name === "outer")).toBe(false);
  });
  it("outside every repo: personal + built-ins", () => {
    const items = slashItems(repos, "D:/elsewhere");
    expect(items.map((i) => i.name).slice(0, 2)).toEqual(["copy-profile", "ir-feedback"]);
    expect(items.length).toBe(2 + BUILTINS.length);
  });
});

describe("slashQuery / slashMatches", () => {
  it("only a leading /word opens the popup", () => {
    expect(slashQuery("/")).toBe("");
    expect(slashQuery("/ir-fe")).toBe("ir-fe");
    expect(slashQuery("/ir feedback now")).toBeNull();
    expect(slashQuery("hello /ir")).toBeNull();
  });
  it("prefix first, then substring, then description; harness before built-ins", () => {
    const items = slashItems(repos, "C:/Workspace/repos/Elly2");
    expect(slashMatches(items, "ir").map((i) => i.name)).toEqual(["ir-feedback", "add-dir"]);
    expect(slashMatches(items, "copy").map((i) => i.name)).toEqual(["copy-profile"]);
    expect(slashMatches(items, "").length).toBe(12);
    expect(slashMatches(items, "zzz")).toEqual([]);
  });
  it("slashCurrent names the typed command and its arguments", () => {
    const items = slashItems(repos, "D:/elsewhere");
    expect(slashCurrent(items, "/ir-feedback")).toMatchObject({ item: { name: "ir-feedback", args: "<period>" }, args: "" });
    expect(slashCurrent(items, "/ir-feedback last week")).toMatchObject({ args: "last week" });
    expect(slashCurrent(items, "/nope x")).toBeNull();
    expect(slashCurrent(items, "plain")).toBeNull();
  });
});
