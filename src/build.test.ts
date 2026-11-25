import { describe, expect, it } from "vitest";
import { buildLine } from "./build";

describe("build line", () => {
  it("names the version and the commit", () => {
    expect(buildLine("0.8.0", "0ec7d31", false)).toBe("Deck 0.8.0 · 0ec7d31");
    expect(buildLine("0.8.0", "0ec7d31+", true)).toBe("Deck 0.8.0 · 0ec7d31+ · dev");
  });
});
