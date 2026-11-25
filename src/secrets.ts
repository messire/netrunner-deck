/**
 * `~/.claude/.env` as PREFERENCES › SECRETS edits it: a KEY / VALUE table
 * over the file's lines. Everything that is not a `KEY=value` line
 * (comments, blank lines, anything unknown) stays as it was, in its place;
 * an edited row replaces its own line, a new row goes at the end. Pure, no
 * I/O: Deck keeps the values only in memory while the panel is open.
 */

export type EnvLine =
  | { kind: "entry"; raw: string; key: string; value: string; exported: boolean; quote: "" | "'" | '"'; comment: string }
  | { kind: "other"; raw: string };

export interface EnvDoc {
  lines: EnvLine[];
  eol: "\n" | "\r\n";
  /** The text ended with a line break. */
  trailing: boolean;
}

/** One row of the table: `line` is its line in the doc, `null` for a new row. */
export interface EnvRow { line: number | null; key: string; value: string }

const KEY = /^[A-Za-z_][A-Za-z0-9_]*$/;

export const validKey = (key: string) => KEY.test(key);

function parseLine(raw: string): EnvLine {
  const other = { kind: "other", raw } as const;
  const m = /^\s*(export\s+)?([A-Za-z_][A-Za-z0-9_]*)\s*=\s*(.*)$/.exec(raw);
  if (!m) return other;
  const exported = !!m[1];
  const key = m[2];
  const rest = m[3];
  const q = rest[0];
  if (q === '"' || q === "'") {
    let value = "";
    let i = 1;
    for (; i < rest.length; i++) {
      const c = rest[i];
      if (c === q) break;
      if (q === '"' && c === "\\" && i + 1 < rest.length) {
        const n = rest[i + 1];
        if (n === "n") { value += "\n"; i++; continue; }
        if (n === '"' || n === "\\") { value += n; i++; continue; }
      }
      value += c;
    }
    // An unclosed quote (a value over several lines): left alone.
    if (i >= rest.length) return other;
    const comment = rest.slice(i + 1);
    if (comment.trim() && !comment.trim().startsWith("#")) return other;
    return { kind: "entry", raw, key, value, exported, quote: q, comment };
  }
  const hash = /\s#/.exec(rest);
  const body = hash ? rest.slice(0, hash.index) : rest;
  const value = body.trimEnd();
  const comment = rest.slice(value.length);
  return { kind: "entry", raw, key, value, exported, quote: "", comment };
}

export function parseEnv(text: string): EnvDoc {
  const eol = text.includes("\r\n") ? "\r\n" : "\n";
  const parts = text.split(/\r?\n/);
  const trailing = text === "" || /\r?\n$/.test(text);
  if (trailing) parts.pop();
  return { lines: parts.map(parseLine), eol, trailing };
}

export function serializeEnv(doc: EnvDoc): string {
  const body = doc.lines.map((l) => l.raw).join(doc.eol);
  return doc.lines.length && doc.trailing ? body + doc.eol : body;
}

/** The value as it is written after `=`: quoted when it has to be (or was). */
export function quoteValue(value: string, was: "" | "'" | '"' = ""): string {
  const needs = /[\s#"'\\]/.test(value);
  if (!needs) return was ? `${was}${value}${was}` : value;
  const multi = value.includes("\n");
  if (was === "'" && !value.includes("'") && !multi) return `'${value}'`;
  if (!value.includes('"') && !value.includes("\\") && !multi) return `"${value}"`;
  if (!value.includes("'") && !multi) return `'${value}'`;
  return `"${value.replace(/\\/g, "\\\\").replace(/"/g, '\\"').replace(/\n/g, "\\n")}"`;
}

/** The rows of the table: one per `KEY=value` line, in order. */
export function rowsOf(doc: EnvDoc): EnvRow[] {
  const rows: EnvRow[] = [];
  doc.lines.forEach((l, i) => { if (l.kind === "entry") rows.push({ line: i, key: l.key, value: l.value }); });
  return rows;
}

/**
 * The file after the table's edits: a row still as it was keeps its line
 * byte for byte, an edited one is written again (its `export` and comment
 * kept), a line whose row is gone is dropped, new rows (with a key) are
 * appended.
 */
export function applyRows(doc: EnvDoc, rows: EnvRow[]): string {
  const byLine = new Map<number, EnvRow>();
  for (const r of rows) if (r.line != null) byLine.set(r.line, r);
  const lines: EnvLine[] = [];
  doc.lines.forEach((l, i) => {
    if (l.kind !== "entry") { lines.push(l); return; }
    const r = byLine.get(i);
    if (!r || !r.key) return;
    if (r.key === l.key && r.value === l.value) { lines.push(l); return; }
    const raw = `${l.exported ? "export " : ""}${r.key}=${quoteValue(r.value, l.quote)}${l.comment}`;
    lines.push(parseLine(raw));
  });
  for (const r of rows) {
    if (r.line == null && r.key) lines.push(parseLine(`${r.key}=${quoteValue(r.value)}`));
  }
  return serializeEnv({ ...doc, lines, trailing: doc.trailing || doc.lines.length === 0 });
}

/**
 * `source` merged into `target` by key: a key both have takes the source's
 * value (in the target's line), a key only the source has is appended, the
 * target's other lines stay as they are. `added` / `replaced` count keys
 * (a key with the same value in both is neither).
 */
export function mergeEnv(target: string, source: string): { text: string; added: number; replaced: number } {
  const from = new Map<string, string>();
  for (const r of rowsOf(parseEnv(source))) from.set(r.key, r.value);
  const doc = parseEnv(target);
  const rows = rowsOf(doc);
  const have = new Set(rows.map((r) => r.key));
  const changed = new Set<string>();
  const merged: EnvRow[] = rows.map((r) => {
    const v = from.get(r.key);
    if (v === undefined || v === r.value) return r;
    changed.add(r.key);
    return { ...r, value: v };
  });
  const fresh = [...from].filter(([k]) => !have.has(k));
  for (const [key, value] of fresh) merged.push({ line: null, key, value });
  return { text: applyRows(doc, merged), added: fresh.length, replaced: changed.size };
}
