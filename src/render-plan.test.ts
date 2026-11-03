import { describe, expect, it } from "vitest";
import { planUpdate } from "./render-plan";

const lines = (n: number, prefix = "l") => Array.from({ length: n }, (_, i) => `${prefix}${i}`);

describe("planUpdate", () => {
  it("first render is a reset with every line", () => {
    expect(planUpdate([], ["a", "b"])).toEqual({ kind: "reset", lines: ["a", "b"] });
  });

  it("identical snapshots need nothing", () => {
    const l = lines(10);
    expect(planUpdate(l, l)).toEqual({ kind: "none" });
  });

  it("pure growth at the end is an append", () => {
    const prev = lines(10);
    expect(planUpdate(prev, [...prev, "new1", "new2"])).toEqual({ kind: "append", lines: ["new1", "new2"] });
  });

  it("a changed last line (streaming reply) is a rewrite from that line", () => {
    const prev = [...lines(8), "● thinking"];
    const next = [...lines(8), "● thinking harder", "  second line"];
    expect(planUpdate(prev, next)).toEqual({ kind: "rewrite", fromLine: 8, lines: ["● thinking harder", "  second line"] });
  });

  it("a snapshot window that slid forward aligns at the shifted offset", () => {
    // prev holds 400 lines 0..399; the pane produced 5 more, Herdr returns 5..404.
    const prev = lines(400);
    const next = [...lines(405).slice(5)];
    expect(planUpdate(prev, next)).toEqual({ kind: "append", lines: ["l400", "l401", "l402", "l403", "l404"] });
  });

  it("history kept beyond the window still aligns (prev longer than next)", () => {
    const prev = lines(1200); // everything ever written
    const next = [...lines(1200).slice(800), "tail"]; // window of the last 400 + one new line
    expect(planUpdate(prev, next)).toEqual({ kind: "append", lines: ["tail"] });
  });

  it("slid window plus a rewritten tail", () => {
    const prev = [...lines(100), "● partial"];
    const next = [...lines(100).slice(3), "● partial done", "✓"];
    expect(planUpdate(prev, next)).toEqual({ kind: "rewrite", fromLine: 100, lines: ["● partial done", "✓"] });
  });

  it("no alignment (another pane's content) resets", () => {
    expect(planUpdate(lines(20, "a"), lines(20, "b"))).toEqual({ kind: "reset", lines: lines(20, "b") });
  });

  it("does not anchor on a couple of blank lines alone", () => {
    const prev = ["", "", "x", "y"];
    const next = ["", "", "p", "q"];
    expect(planUpdate(prev, next).kind).toBe("reset");
  });

  it("a short snapshot that matches entirely is an alignment", () => {
    expect(planUpdate(["❯ hi", "● hello"], ["❯ hi", "● hello"])).toEqual({ kind: "none" });
    expect(planUpdate(["❯ hi"], ["❯ hi", "● hello"])).toEqual({ kind: "append", lines: ["● hello"] });
  });

  it("prefers the longest match, and the latest offset among equals", () => {
    // prev repeats a block; the snapshot continues from the second copy.
    const block = ["a", "b", "c", "d"];
    const prev = [...block, "x", ...block];
    expect(planUpdate(prev, [...block, "new"])).toEqual({ kind: "append", lines: ["new"] });
    // A longer match wins even when it is earlier.
    expect(planUpdate([...block, "e", "x", ...block], [...block, "e", "new"])).toEqual({ kind: "rewrite", fromLine: 5, lines: ["new"] });
  });

  it("a shrunken snapshot rewrites from the divergence", () => {
    const prev = lines(10);
    const next = lines(6);
    expect(planUpdate(prev, next)).toEqual({ kind: "rewrite", fromLine: 6, lines: [] });
  });
});
