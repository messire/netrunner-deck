import { describe, expect, it } from "vitest";
import { applyRows, mergeEnv, parseEnv, quoteValue, rowsOf, serializeEnv, validKey } from "./secrets";

const FILE = [
  "# fake values only",
  "",
  "FAKE_TOKEN=abc123",
  "export FAKE_URL=\"http://example.invalid/a b\"",
  "QUOTED='it has # inside'",
  "WITH_COMMENT=plain # trailing note",
  "not a key value line",
  "EMPTY=",
  "",
].join("\n");

describe("parseEnv", () => {
  it("round-trips the text as it is", () => {
    expect(serializeEnv(parseEnv(FILE))).toBe(FILE);
    expect(serializeEnv(parseEnv("A=1"))).toBe("A=1");
    expect(serializeEnv(parseEnv(""))).toBe("");
    const crlf = FILE.replace(/\n/g, "\r\n");
    expect(serializeEnv(parseEnv(crlf))).toBe(crlf);
  });

  it("reads keys and values", () => {
    expect(rowsOf(parseEnv(FILE)).map((r) => [r.key, r.value])).toEqual([
      ["FAKE_TOKEN", "abc123"],
      ["FAKE_URL", "http://example.invalid/a b"],
      ["QUOTED", "it has # inside"],
      ["WITH_COMMENT", "plain"],
      ["EMPTY", ""],
    ]);
  });

  it("unescapes double quotes and leaves an unclosed quote alone", () => {
    const d = parseEnv('A="x \\"y\\" \\\\ z"\nB="open\nC=1\r\n');
    expect(rowsOf(d).map((r) => [r.key, r.value])).toEqual([["A", 'x "y" \\ z'], ["C", "1"]]);
    expect(d.lines[1].kind).toBe("other");
  });
});

describe("applyRows", () => {
  it("changes nothing when nothing changed", () => {
    const d = parseEnv(FILE);
    expect(applyRows(d, rowsOf(d))).toBe(FILE);
  });

  it("writes an edited row in its own line, keeping export and the comment", () => {
    const d = parseEnv(FILE);
    const rows = rowsOf(d).map((r) =>
      r.key === "FAKE_URL" ? { ...r, value: "no-spaces" }
      : r.key === "WITH_COMMENT" ? { ...r, key: "RENAMED", value: "has space" }
      : r);
    const out = applyRows(d, rows).split("\n");
    expect(out[3]).toBe('export FAKE_URL="no-spaces"');
    expect(out[5]).toBe('RENAMED="has space" # trailing note');
    expect(out[6]).toBe("not a key value line");
  });

  it("drops deleted rows and appends new ones at the end", () => {
    const d = parseEnv(FILE);
    const rows = rowsOf(d).filter((r) => r.key !== "FAKE_TOKEN");
    rows.push({ line: null, key: "NEW_ONE", value: "v w" }, { line: null, key: "", value: "ignored" });
    const out = applyRows(d, rows);
    expect(out).not.toContain("FAKE_TOKEN");
    expect(out.endsWith('EMPTY=\nNEW_ONE="v w"\n')).toBe(true);
    expect(out.startsWith("# fake values only\n\nexport FAKE_URL")).toBe(true);
  });

  it("keeps CRLF and starts a new file with a line break", () => {
    const d = parseEnv("A=1\r\nB=2\r\n");
    const rows = rowsOf(d);
    rows[1] = { ...rows[1], value: "3" };
    expect(applyRows(d, [...rows, { line: null, key: "C", value: "4" }])).toBe("A=1\r\nB=3\r\nC=4\r\n");
    expect(applyRows(parseEnv(""), [{ line: null, key: "X", value: "1" }])).toBe("X=1\n");
  });

  it("round-trips values that need quoting", () => {
    for (const v of ["a b", "a#b", "say \"hi\"", "it's", "both ' and \"", "back\\slash", "two\nlines", ""]) {
      const out = applyRows(parseEnv(""), [{ line: null, key: "K", value: v }]);
      expect(rowsOf(parseEnv(out))[0].value).toBe(v);
    }
  });
});

describe("quoteValue and validKey", () => {
  it("quotes only when needed, keeping the quote it had", () => {
    expect(quoteValue("plain")).toBe("plain");
    expect(quoteValue("plain", "'")).toBe("'plain'");
    expect(quoteValue("a b", "'")).toBe("'a b'");
    expect(quoteValue("a b")).toBe('"a b"');
    expect(quoteValue('say "hi"')).toBe(`'say "hi"'`);
  });

  it("accepts environment names", () => {
    expect(validKey("FAKE_KEY_1")).toBe(true);
    expect(validKey("_x")).toBe(true);
    expect(["", "1A", "A-B", "A B", "export A"].some(validKey)).toBe(false);
  });
});

describe("mergeEnv", () => {
  it("replaces shared keys in place, appends new ones, keeps the rest", () => {
    const target = "# target\nKEEP=1\nSHARED=old # note\nSAME=x\n";
    const source = "# source comment\nSHARED=new value\nSAME=x\nNEW_KEY=2\n";
    const m = mergeEnv(target, source);
    expect(m).toEqual({ text: '# target\nKEEP=1\nSHARED="new value" # note\nSAME=x\nNEW_KEY=2\n', added: 1, replaced: 1 });
  });

  it("works into an empty file and changes nothing when nothing differs", () => {
    expect(mergeEnv("", "A=1\r\nB=2\r\n")).toEqual({ text: "A=1\nB=2\n", added: 2, replaced: 0 });
    expect(mergeEnv("A=1\n", "A=1\n")).toEqual({ text: "A=1\n", added: 0, replaced: 0 });
  });
});
