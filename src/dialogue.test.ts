import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { classify, promptLine, stripAnsi, stripBackground, blockWidth, type LineKind } from "./dialogue";

const FIXTURES = new URL("./__fixtures__/pane-read/", import.meta.url);
function loadFixture(name: string) {
  const raw = readFileSync(new URL(`${name}.ans`, FIXTURES), "utf8").replace(/\r\n?/g, "\n").split("\n");
  const expected = JSON.parse(readFileSync(new URL(`${name}.expected.json`, FIXTURES), "utf8")) as {
    blocks: { kind: LineKind; from: number; to: number }[];
    neverFramed: number[];
    promptLine: number;
  };
  return { raw, plain: raw.map(stripAnsi), expected };
}

describe("classify on synthetic Claude Code snapshot", () => {
  const { raw, plain, expected } = loadFixture("synthetic-claude-basic");
  const kinds = classify(plain, raw);

  for (const b of expected.blocks) {
    it(`frames ${b.kind} block lines ${b.from}-${b.to}`, () => {
      for (let i = b.from; i <= b.to; i++) expect(kinds[i], `line ${i}: ${plain[i]}`).toBe(b.kind);
    });
  }
  it("leaves tool calls, results, dim summaries, errors and chrome unframed", () => {
    for (const i of expected.neverFramed) expect(kinds[i], `line ${i}: ${plain[i]}`).toBeNull();
  });
  it("finds the input prompt line", () => {
    expect(promptLine(plain)).toBe(expected.promptLine);
  });
  it("does not frame the live prompt even though it starts with ❯", () => {
    expect(kinds[expected.promptLine]).toBeNull();
  });
  it.fails("known gap: dim 24-bit summary starting with a digit stays outside the frame (R12)", () => {
    expect(kinds[18]).toBeNull();
  });
});

describe("helpers", () => {
  it("stripAnsi removes SGR and OSC sequences", () => {
    expect(stripAnsi("\x1b[1m\x1b[38;2;1;2;3mhi\x1b[0m")).toBe("hi");
    expect(stripAnsi("\x1b]0;title\x07x")).toBe("x");
  });
  it("stripBackground removes only background colours", () => {
    expect(stripBackground("\x1b[48;2;9;9;9m\x1b[33mtext\x1b[0m")).toBe("\x1b[33mtext\x1b[0m");
  });
  it("blockWidth is the widest trimmed line plus padding, capped at cols", () => {
    expect(blockWidth(["ab   ", "abcd"], 80)).toBe(6);
    expect(blockWidth(["x".repeat(200)], 80)).toBe(80);
  });
  it.fails("dim summary starting with a digit is not a continuation (audit #10)", () => {
    const raw = ["\x1b[0m\x1b[38;2;155;232;234m● \x1b[0mReply", "  \x1b[1m\x1b[38;5;240m8 files changed\x1b[0m"];
    const kinds = classify(raw.map(stripAnsi), raw);
    expect(kinds[1]).toBeNull();
  });
});
