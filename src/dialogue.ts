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
const TOOL_SUMMARY = /^(Ran|Read|Wrote|Updated|Searched|Found|Listed|Fetched)\b.*\b(commands?|files?|lines?|matches|results?)\b/i;

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

const isContinuation = (line: string) => /^\s{2,}\S/.test(line);
const isBlank = (line: string) => line.trim() === "";

export function classify(lines: string[]): LineKind[] {
  const kinds: LineKind[] = new Array(lines.length).fill(null);
  let current: LineKind = null;
  for (let i = 0; i < lines.length; i++) {
    const line = lines[i];
    const start = startKind(line);
    if (start) {
      current = start;
      kinds[i] = start;
      continue;
    }
    if (!current) continue;
    if (isContinuation(line)) {
      kinds[i] = current;
    } else if (isBlank(line) && i + 1 < lines.length && isContinuation(lines[i + 1])) {
      kinds[i] = current; // paragraph break inside the same message
    } else {
      current = null;
    }
  }
  return kinds;
}

/** Index of the line that carries the input prompt, if the CLI shows one. */
export function promptLine(lines: string[]): number {
  for (let i = lines.length - 1; i >= 0; i--) {
    if (/^[❯›>]\s*$/.test(lines[i]) || /^[❯›]\s/.test(lines[i])) return i;
  }
  return -1;
}
