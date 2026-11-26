import { describe, expect, it } from "vitest";
import { codeBody, codeSpans } from "./codespans";

describe("codeSpans", () => {
  it("finds inline code and keeps every character", () => {
    const t = "run `npm test` then `git push` ok";
    const s = codeSpans(t);
    expect(s.map((x) => x.kind)).toEqual(["text", "code", "text", "code", "text"]);
    expect(s.map((x) => x.text).join("")).toBe(t);
    expect(codeBody(s[1])).toBe("npm test");
  });
  it("fenced blocks win over inline spans inside them", () => {
    const t = "answer:\n```\n№ n. `chat`\n - x\n```\nend";
    const s = codeSpans(t);
    expect(s.map((x) => x.kind)).toEqual(["text", "block", "text"]);
    expect(codeBody(s[1])).toBe("№ n. `chat`\n - x");
    expect(s.map((x) => x.text).join("")).toBe(t);
  });
  it("leaves an unclosed or multi-line backtick alone", () => {
    expect(codeSpans("a `b").map((x) => x.kind)).toEqual(["text"]);
    expect(codeSpans("a `b\nc` d").map((x) => x.kind)).toEqual(["text"]);
  });
});
