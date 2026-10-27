import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { blockWidth, classify, defaultWidthOf, fitRules, hasBackground, inputBoxStart, promptLine, snapshotCols, stripAnsi, stripBackground, type LineKind } from "./dialogue";

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

const SNAPSHOTS = [
  "synthetic-claude-basic", // > echo, reply + continuation, dim "Read 2 files" summary, tool calls, digit summary, red error
  "synthetic-claude-long-reply", // paragraphs, bullet and numbered lists, dim summaries starting with a digit / "Called"
  "synthetic-claude-tools", // tool call/result pairs with ⎿, multi-line results, red ● error, 24-bit red system note, ✻ status
  "synthetic-claude-prompt-wrap", // ❯ user echo, wrapped prompt, CJK/emoji reply, empty live prompt
];

describe.each(SNAPSHOTS)("classify on synthetic Claude Code snapshot %s", (name) => {
  const { raw, plain, expected } = loadFixture(name);
  const kinds = classify(plain, raw);

  for (const b of expected.blocks) {
    it(`frames ${b.kind} block lines ${b.from}-${b.to}`, () => {
      for (let i = b.from; i <= b.to; i++) expect(kinds[i], `line ${i}: ${plain[i]}`).toBe(b.kind);
    });
    it(`ends the ${b.kind} block at line ${b.to}`, () => {
      // The block must not bleed into the next line (blank-before-summary, tool call, status…).
      if (b.to + 1 < kinds.length) expect(kinds[b.to + 1], `line ${b.to + 1}: ${plain[b.to + 1]}`).not.toBe(b.kind);
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
  it("classifies the same way without the trailing empty line left by the final CRLF", () => {
    const trimmed = classify(plain.slice(0, -1), raw.slice(0, -1));
    expect(trimmed).toEqual(kinds.slice(0, -1));
  });
});

const AGENT = "\x1b[0m\x1b[38;2;155;232;234m● \x1b[0m";
const run = (raw: string[]) => classify(raw.map(stripAnsi), raw);

describe("leadStyle (R12): first printable char is found past ANSI and whitespace", () => {
  it("24-bit low-luminance summary starting with a digit is not a continuation", () => {
    const kinds = run([`${AGENT}Reply`, "  \x1b[0m\x1b[38;2;94;68;84m8 files changed\x1b[0m"]);
    expect(kinds).toEqual(["agent", null]);
  });
  it("bold before the 24-bit dim colour does not hide the dim", () => {
    const kinds = run([`${AGENT}Reply`, "  \x1b[1m\x1b[38;2;94;68;84m3 files changed\x1b[0m"]);
    expect(kinds[1]).toBeNull();
  });
  it("256-colour grey (SGR 38;5;240) and SGR 2 / 90 are dim", () => {
    for (const sgr of ["\x1b[1m\x1b[38;5;240m", "\x1b[2m", "\x1b[90m", "\x1b[38;5;8m"]) {
      const kinds = run([`${AGENT}Reply`, `  ${sgr}8 files changed\x1b[0m`]);
      expect(kinds[1], sgr).toBeNull();
    }
  });
  it("a reset after a dim colour clears it (the text itself is default colour)", () => {
    const kinds = run([`${AGENT}Reply`, "  \x1b[2m\x1b[0mplain continuation"]);
    expect(kinds[1]).toBe("agent");
  });
  it("bright 24-bit colour and bold are not dim", () => {
    const kinds = run([`${AGENT}Reply`, "  \x1b[38;2;230;230;230mstill the reply", "  \x1b[1mbold continuation\x1b[0m"]);
    expect(kinds).toEqual(["agent", "agent", "agent"]);
  });
  it("blank line before a 24-bit dim summary is outside the block", () => {
    const kinds = run([`${AGENT}Reply`, "", "  \x1b[0m\x1b[38;2;94;68;84m2 files changed\x1b[0m"]);
    expect(kinds).toEqual(["agent", null, null]);
  });
  it("blank line before a plain continuation stays inside the block", () => {
    const kinds = run([`${AGENT}Reply`, "", "  second paragraph"]);
    expect(kinds).toEqual(["agent", "agent", "agent"]);
  });
  it("red ● lines are errors, not replies: SGR 31, 91 and 24-bit red", () => {
    for (const sgr of ["\x1b[31m", "\x1b[91m", "\x1b[38;2;255;43;74m", "\x1b[0m\x1b[38;2;255;107;74m"]) {
      const kinds = run([`${sgr}● \x1b[0mError: something failed`]);
      expect(kinds[0], sgr).toBeNull();
    }
  });
  it("24-bit colours with high G or B are not red", () => {
    for (const sgr of ["\x1b[38;2;255;200;100m", "\x1b[38;2;200;100;200m", "\x1b[38;2;170;130;60m"]) {
      const kinds = run([`${sgr}● \x1b[0mA reply in an unusual theme colour`]);
      expect(kinds[0], sgr).toBe("agent");
    }
  });
  it("a dim ● line (system note) is not a reply", () => {
    expect(run(["\x1b[0m\x1b[38;2;94;68;84m● \x1b[0mContext compacted"])).toEqual([null]);
  });
});

describe("promptLine", () => {
  it("finds a live prompt with NBSP and a dim placeholder", () => {
    const raw = [`${AGENT}Done.`, "", "❯  \x1b[0m\x1b[2mtype a message\x1b[0m"];
    const plain = raw.map(stripAnsi);
    expect(promptLine(plain)).toBe(2);
    expect(classify(plain, raw)[2]).toBeNull();
  });
  it("prefers the last prompt-looking line so a ❯ user echo above is still a user turn", () => {
    const raw = ["\x1b[0m\x1b[38;2;94;68;84m❯ \x1b[0mdo it", "", `${AGENT}Done.`, "", "❯ "];
    const plain = raw.map(stripAnsi);
    expect(promptLine(plain)).toBe(4);
    expect(classify(plain, raw)[0]).toBe("user");
  });
  it("returns -1 without a prompt", () => {
    expect(promptLine(["> hi", "● hello"])).toBe(-1);
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
});

describe("blockWidth (R13'): display cells, not code points", () => {
  it("is the widest trimmed line plus padding, capped at cols", () => {
    expect(blockWidth(["ab   ", "abcd"], 80)).toBe(6);
    expect(blockWidth(["x".repeat(200)], 80)).toBe(80);
  });
  it("counts CJK as two cells", () => {
    expect(blockWidth(["日本語"], 80)).toBe(8);
    expect(blockWidth(["한글"], 80)).toBe(6);
    expect(blockWidth(["ｱｲｳ"], 80)).toBe(5); // halfwidth katakana (FF61+) stay narrow
    expect(blockWidth(["ＡＢ"], 80)).toBe(6); // fullwidth latin
  });
  it("counts emoji as two cells and combining marks / VS16 as zero", () => {
    expect(blockWidth(["Готово ✅ 日本語"], 80)).toBe(6 + 1 + 2 + 1 + 6 + 2);
    expect(blockWidth(["😀"], 80)).toBe(4);
    expect(blockWidth(["é"], 80)).toBe(3);
    expect(blockWidth(["✔"], 80)).toBe(3); // text-presentation dingbat: 1 cell
    expect(blockWidth(["✔️"], 80)).toBe(4); // emoji presentation via VS16: 2 cells
    expect(blockWidth(["👨‍💻"], 80)).toBe(6); // ZWJ is zero width; the parts stay 2+2 (no grapheme merging)
  });
  it("accepts an injected width function", () => {
    expect(blockWidth(["abc"], 80, () => 3)).toBe(11);
    expect(blockWidth(["日本語"], 80, () => 1)).toBe(5);
  });
  it("defaultWidthOf table", () => {
    expect(defaultWidthOf("a")).toBe(1);
    expect(defaultWidthOf("中")).toBe(2);
    expect(defaultWidthOf("あ")).toBe(2);
    expect(defaultWidthOf("ガ")).toBe(2);
    expect(defaultWidthOf("́")).toBe(0);
    expect(defaultWidthOf("️")).toBe(0);
    expect(defaultWidthOf("🧪")).toBe(2);
    expect(defaultWidthOf("✅")).toBe(2);
    expect(defaultWidthOf("☀")).toBe(1);
    expect(defaultWidthOf("☀", "️")).toBe(2);
    expect(defaultWidthOf("─")).toBe(1);
  });
});

describe("classify: user turn on Claude Code's background band (v2.1+)", () => {
  const BG = "\x1b[48;2;30;10;20m";
  const DIM = "\x1b[38;2;94;68;84m";
  const FG = "\x1b[38;2;155;232;234m";
  const band = (text: string) => `\x1b[0m${BG}  \x1b[0m${FG}${BG}${text}\x1b[0m${BG}        `;
  const blank = `\x1b[0m${BG}                    `;
  const raw = [
    `\x1b[0m${DIM}${BG}❯ \x1b[0m${FG}${BG}# Редизайн\x1b[0m${BG}      `,
    blank,
    band("Необходимо переработать интерфейс"),
    blank,
    band("| col | col |"),
    band("│ table row │"),
    band("- bullet"),
    band("```"),
    blank,
    "",
    `\x1b[0m${FG}● \x1b[0mПринял.`,
    "  1. Волью main.",
    "",
    "❯ ",
  ];
  const plain = raw.map(stripAnsi);
  it("every row with the band belongs to the user turn, blanks and tables included", () => {
    const kinds = classify(plain, raw);
    expect(kinds.slice(0, 9)).toEqual(new Array(9).fill("user"));
  });
  it("the turn ends exactly where the band ends", () => {
    const kinds = classify(plain, raw);
    expect(kinds[9]).toBeNull();
    expect(kinds[10]).toBe("agent");
    expect(kinds[11]).toBe("agent");
    expect(kinds[13]).toBeNull();
  });
  it("without a band (older Claude Code) the indentation rule still applies", () => {
    const old = ["❯ hello", "  more", "not part", "", "● reply", "", "❯ "];
    expect(classify(old, old)).toEqual(["user", "user", null, null, "agent", null, null]);
  });
  it("hasBackground sees 24-bit, 256-colour and classic backgrounds only", () => {
    expect(hasBackground(BG + "x")).toBe(true);
    expect(hasBackground("\x1b[48;5;236mx")).toBe(true);
    expect(hasBackground("\x1b[44mx")).toBe(true);
    expect(hasBackground("\x1b[38;2;30;10;20mx")).toBe(false);
    expect(hasBackground(undefined)).toBe(false);
  });
});

describe("snapshotCols / fitRules: a snapshot drawn for a wider PTY", () => {
  it("snapshotCols is the widest line in cells", () => {
    expect(snapshotCols(["ab", "─".repeat(169), "  x"])).toBe(169);
    expect(snapshotCols(["日本語", "ab"])).toBe(6);
    expect(snapshotCols([])).toBe(0);
  });
  it("a rule-only line wider than the view is cut to the view width, keeping its colour", () => {
    const line = "\x1b[38;2;192;19;42m" + "─".repeat(169) + "\x1b[0m";
    const out = fitRules(line, 100);
    expect(stripAnsi(out)).toBe("─".repeat(100));
    expect(out.startsWith("\x1b[38;2;192;19;42m")).toBe(true);
  });
  it("indentation counts: 2 spaces + 167 rules at 100 cols → 2 spaces + 98 rules", () => {
    expect(stripAnsi(fitRules("  " + "─".repeat(167), 100))).toBe("  " + "─".repeat(98));
  });
  it("lines that fit, or that carry text, are untouched", () => {
    expect(fitRules("─".repeat(50), 100)).toBe("─".repeat(50));
    expect(fitRules("── title ──" + "─".repeat(200), 100)).toBe("── title ──" + "─".repeat(200));
    expect(fitRules("● text", 100)).toBe("● text");
  });
  it("only the longest run is cut when a line has several", () => {
    expect(stripAnsi(fitRules("──  " + "─".repeat(120), 100))).toBe("──  " + "─".repeat(96));
  });
});

describe("inputBoxStart: the pinned input area", () => {
  it("starts at the rule above the live prompt and runs to the end", () => {
    const lines = ["● reply", "", "─".repeat(40), "❯ ", "─".repeat(40), "  ⏵⏵ bypass permissions on"];
    expect(inputBoxStart(lines)).toBe(2);
  });
  it("falls back to the prompt line when there is no rule above it", () => {
    expect(inputBoxStart(["● reply", "", "❯ typing"])).toBe(2);
    expect(inputBoxStart(["● reply", "x", "", "❯ "])).toBe(3);
  });
  it("a rule further than two lines up is not the box", () => {
    expect(inputBoxStart(["─".repeat(10), "a", "b", "c", "❯ "])).toBe(4);
  });
  it("-1 without a live prompt", () => {
    expect(inputBoxStart(["● reply", "", "$ "])).toBe(-1);
    expect(inputBoxStart([])).toBe(-1);
  });
  it("a user echo above the live prompt is not the input box", () => {
    const lines = ["❯ my question", "", "● reply", "", "─".repeat(40), "❯ ", "─".repeat(40)];
    expect(inputBoxStart(lines)).toBe(4);
  });
});
