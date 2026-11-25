/**
 * Clone addresses for OPEN REPOSITORY (no DOM).
 */

// ---- clone addresses ---------------------------------------------------------------

/** A git address to clone: https/http/ssh/git URL or scp-like `user@host:path`. */
export function isGitUrl(text: string): boolean {
  const s = text.trim();
  if (!s || /\s/.test(s)) return false;
  const m = /^(https?|ssh|git|git\+ssh):\/\/([^/]+)\/(.+)$/.exec(s);
  if (m) return !!m[2] && !!m[3].replace(/\/+$/, "");
  const scp = /^([^@/\\:]+)@([^/\\:]+):(.+)$/.exec(s);
  return !!scp && !!scp[3].replace(/^\/+|\/+$/g, "");
}

/** The folder a clone gets: the last path part without `.git`. */
export function repoNameOf(url: string): string {
  const last = url.trim().replace(/\/+$/, "").split(/[/:]/).pop() ?? "";
  return last.replace(/\.git$/, "") || "repo";
}

/** A github.com address (any form): GitHub sign-in can help when it is refused. */
export const isGitHub = (url: string) => /^(https?:\/\/([^@/]+@)?(www\.)?github\.com\/|ssh:\/\/([^@/]+@)?github\.com[/:]|[^@/]+@github\.com:)/i.test(url.trim());

/** One clone status as the dialog line says it. */
export function cloneLine(s: { phase?: string; percent?: number | null }): string {
  const phase = (s.phase ?? "").trim() || "Cloning";
  return typeof s.percent === "number" ? `${phase} · ${s.percent}%` : `${phase}…`;
}
