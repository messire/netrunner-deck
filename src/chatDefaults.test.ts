import { describe, expect, it } from "vitest";
import { defaultsParams, modelChoices, newPanes } from "./chatDefaults";

describe("chat defaults", () => {
  it("sets only what is not left to the CLI", () => {
    expect(defaultsParams({ model: null, mode: null })).toBeNull();
    expect(defaultsParams({ model: "claude-opus-5-5", mode: "bypassPermissions" })).toEqual({ model: "claude-opus-5-5", permission_mode: "bypassPermissions" });
    expect(defaultsParams({ model: null, mode: "plan" })).toEqual({ permission_mode: "plan" });
  });

  it("finds panes that appeared on a server already seen", () => {
    const first = newPanes(new Map(), [{ pane: "a|p1", server: "a" }]);
    expect(first.fresh).toEqual([]);
    const second = newPanes(first.seen, [{ pane: "a|p1", server: "a" }, { pane: "a|p2", server: "a" }, { pane: "b|p1", server: "b" }]);
    expect(second.fresh).toEqual(["a|p2"]);
    expect(newPanes(second.seen, [{ pane: "b|p2", server: "b" }]).fresh).toEqual(["b|p2"]);
  });

  it("offers aliases, listed models and the saved one once each", () => {
    expect(modelChoices(["default", "claude-opus-5-5", "opus"], "my-model")).toEqual(["opus", "sonnet", "haiku", "claude-opus-5-5", "my-model"]);
  });
});
