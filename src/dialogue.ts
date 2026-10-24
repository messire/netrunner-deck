/**
 * Classifies agent-CLI screen lines into dialogue blocks so the terminal view
 * can frame them: the user's prompts and the agent's prose replies. Tool calls,
 * results, spinners and everything else stay untouched.
 *
 * Heuristics follow Claude Code's layout: a user message is echoed as "> text",
 * an assistant reply starts with "● text", tool calls look like "● Read(…)" /
 * "Ran 2 shell commands", results start with "⎿". Continuation lines of a
 * message are indented; a block ends at the first unindented, non-blank line.
 */
export type LineKind = "user" | "agent" | null;

const ANSI = /\x1b\[[0-9;?]*[ -/]*[@-~]|\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)|\x1b[@-Z\\-_]/g;
export const stripAnsi = (s: string) => s.replace(ANSI, "");

const TOOL_CALL = /^[A-Za-z_][\w.-]*\s*\(/;
// Collapsed tool lines ("Ran 2 shell commands", "Searched for …", "Called Slack …").
const TOOL_SUMMARY = /^(Ran|Read|Wrote|Updated|Edited|Searched|Found|Listed|Fetched|Called|Created|Deleted|Checked|Queried|Opened|Viewed|Loaded|Executed|Invoked|Used)\b/;

function startKind(line: string): LineKind {
  const user = /^>\s+(\S.*)?$/.exec(line);
  if (user) return "user";
  const agent = /^[●⏺]\s+(.*)$/.exec(line);
  if (agent) {
    const rest = agent[1] ?? "";
    if (TOOL_CALL.test(rest) || TOOL_SUMMARY.test(rest)) return null;
    return "agent";
  }
  return null;
}

// Indented lines continue a message, except tool summaries ("Ran 2 shell
// commands"), tool results ("⎿ …") and status lines that Claude Code also indents.
const isContinuation = (line: string) =>
  /^\s{2,}\S/.test(line) && !TOOL_SUMMARY.test(line.trim()) && !/^\s*[⎿└├│]/.test(line) && !/^\s*[※✻✽✶✳·]\s/.test(line);
const isBlank = (line: string) => line.trim() === "";

/** `raw` (with ANSI) is optional: dim-styled indented lines are tool summaries, never message text. */
export function classify(lines: string[], raw?: string[]): LineKind[] {
  const kinds: LineKind[] = new Array(lines.length).fill(null);
  let current: LineKind = null;
  for (let i = 0; i < lines.length; i++) {
    const line = lines[i];
    const dim = raw ? /^(?:\s|\x1b\[[0-9;]*m)*\x1b\[(?:2|90|38;5;(?:8|24[0-9]|25[0-5]))m/.test(raw[i]) : false;
    const start = startKind(line);
    if (start) {
      current = start;
      kinds[i] = start;
      continue;
    }
    if (!current) continue;
    if (isContinuation(line) && !dim) {
      kinds[i] = current;
    } else if (isBlank(line) && i + 1 < lines.length && isContinuation(lines[i + 1]) && !(raw && /\x1b\[(?:2|90)m/.test(raw[i + 1]))) {
      kinds[i] = current; // paragraph break inside the same message
    } else {
      current = null;
    }
  }
  return kinds;
}

/** Index of the line that carries the input prompt, if the CLI shows one. */
/** Display width of a block: its widest line, in cells, capped at `cols`. */
export function blockWidth(lines: string[], cols: number): number {
  let w = 0;
  for (const line of lines) w = Math.max(w, [...line.replace(/\s+$/, "")].length);
  return Math.min(cols, w + 2);
}

export function promptLine(lines: string[]): number {
  for (let i = lines.length - 1; i >= 0; i--) {
    if (/^[❯›>]\s*$/.test(lines[i]) || /^[❯›]\s/.test(lines[i])) return i;
  }
  return -1;
}
