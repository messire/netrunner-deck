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

const DIM = /\x1b\[(?:2|90|38;5;(?:8|24[0-9]|25[0-5]))m/;
const RED = /\x1b\[(?:31|91|38;5;(?:1|9|196|160|203)|38;2;(?:1[89]\d|2\d\d);(?:\d|[1-9]\d|1[01]\d);\d+)m/;

/** Style of the first styled run of a line, before any printable text. */
function leadStyle(raw: string | undefined, plain: string): { dim: boolean; red: boolean } {
  if (!raw) return { dim: false, red: false };
  const head = raw.slice(0, Math.max(0, raw.indexOf(plain.trim().slice(0, 1) || "\u0000")) + 8);
  return { dim: DIM.test(head), red: RED.test(head) };
}

function startKind(line: string, raw: string | undefined, isPrompt: boolean): LineKind {
  // Claude Code echoes the user's turn as "> text" (older) or "❯ text" (newer);
  // the live input line also starts with "❯", so it is excluded by index.
  if (!isPrompt && /^[>❯›]\s+\S/.test(line)) return "user";
  const agent = /^[●⏺]\s+(.*)$/.exec(line);
  if (agent) {
    const rest = agent[1] ?? "";
    if (TOOL_CALL.test(rest) || TOOL_SUMMARY.test(rest)) return null;
    const style = leadStyle(raw, line);
    if (style.dim || style.red) return null; // system notes and errors are not replies
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
  const prompt = promptLine(lines);
  let current: LineKind = null;
  for (let i = 0; i < lines.length; i++) {
    const line = lines[i];
    const dim = raw ? leadStyle(raw[i], line).dim : false;
    const start = startKind(line, raw?.[i], i === prompt);
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
