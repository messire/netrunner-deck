import { describe, expect, it } from "vitest";
import { createLatestGate } from "./latest";

describe("createLatestGate", () => {
  it("numbers requests per key starting at 1", () => {
    const gate = createLatestGate();
    expect(gate.begin("a")).toBe(1);
    expect(gate.begin("a")).toBe(2);
    expect(gate.begin("b")).toBe(1);
  });

  it("only the latest sequence for a key is current", () => {
    const gate = createLatestGate();
    const s1 = gate.begin("pane-1");
    const s2 = gate.begin("pane-1");
    expect(gate.isCurrent("pane-1", s1)).toBe(false);
    expect(gate.isCurrent("pane-1", s2)).toBe(true);
  });

  it("keys are independent", () => {
    const gate = createLatestGate();
    const a = gate.begin("a");
    const b = gate.begin("b");
    gate.begin("b");
    expect(gate.isCurrent("a", a)).toBe(true);
    expect(gate.isCurrent("b", b)).toBe(false);
  });

  it("unknown key or stale/foreign seq is never current", () => {
    const gate = createLatestGate();
    expect(gate.isCurrent("nope", 1)).toBe(false);
    gate.begin("a");
    expect(gate.isCurrent("a", 0)).toBe(false);
    expect(gate.isCurrent("a", 2)).toBe(false);
  });

  it("drops an older response that resolves after a newer one (race)", async () => {
    const gate = createLatestGate();
    const applied: string[] = [];
    const request = (key: string, text: string, delayMs: number) => {
      const seq = gate.begin(key);
      return new Promise<void>((resolve) =>
        setTimeout(() => {
          if (gate.isCurrent(key, seq)) applied.push(text);
          resolve();
        }, delayMs),
      );
    };
    // Slow first request, fast second one for the same key: the slow one is stale.
    await Promise.all([request("p", "old", 20), request("p", "new", 1)]);
    expect(applied).toEqual(["new"]);
  });
});
