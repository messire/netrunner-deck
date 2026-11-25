import { describe, expect, it } from "vitest";
import { cloneLine, isGitHub, isGitUrl, repoNameOf } from "./transfer";

describe("clone addresses", () => {
  it("git addresses are recognised, paths and names are not", () => {
    for (const u of ["https://github.com/o/atlas.git", "git@github.com:o/atlas.git", "ssh://git@example.com:2222/o/r", "git://example.com/r", "me@example.com:repos/r"]) expect(isGitUrl(u), u).toBe(true);
    for (const u of ["", "atlas", "~/atlas", "C:\\Workspace\\atlas", "/home/me/atlas", "https://github.com", "git@github.com:", "a b@c:d"]) expect(isGitUrl(u), u).toBe(false);
  });
  it("the folder is named after the repository", () => {
    expect(repoNameOf("https://github.com/o/atlas.git")).toBe("atlas");
    expect(repoNameOf("git@github.com:o/notes")).toBe("notes");
    expect(repoNameOf("https://example.com/a/b/")).toBe("b");
  });
  it("GitHub addresses in every form", () => {
    for (const u of ["https://github.com/o/r", "git@github.com:o/r.git", "ssh://git@github.com/o/r", "https://me@github.com/o/r"]) expect(isGitHub(u), u).toBe(true);
    for (const u of ["https://gitlab.com/o/r", "git@example.com:o/r", "https://notgithub.com/o/r"]) expect(isGitHub(u), u).toBe(false);
  });
  it("progress reads as phase and percent", () => {
    expect(cloneLine({ phase: "Receiving objects", percent: 45 })).toBe("Receiving objects · 45%");
    expect(cloneLine({ phase: "", percent: null })).toBe("Cloning…");
  });
});
