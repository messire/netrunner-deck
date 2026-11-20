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
const TOOL_SUMMARY = /^(Ran|Read|Wrote|Updated|Edited|Searched|Found|Listed|Fetched|Called|Created|Deleted|Checked|Queried|Opened|Viewed|Loaded|Executed|Invoked|Used|Running|Reading|Writing|Updating|Editing|Searching|Fetching|Calling|Listing|Checking|Querying|Thinking|Waiting)\b/;

// Everything before the first printable character: SGR/CSI sequences and whitespace.
const LEAD = /^(?:\x1b\[[0-9;?]*[ -/]*[@-~]|\s)*/;
const SGR = /\x1b\[([0-9;]*)m/g;
// 24-bit colours: Claude Code's "dim" is a theme colour (e.g. 94;68;84), so dim
// means low relative luminance; red means a strongly red-dominant colour.
const DIM_LUMINANCE = 0.35;
const isDimRgb = (r: number, g: number, b: number) => (0.2126 * r + 0.7152 * g + 0.0722 * b) / 255 < DIM_LUMINANCE;
const isRedRgb = (r: number, g: number, b: number) => r > 180 && g < 120 && b < 120;
const DIM_256 = (n: number) => n === 8 || (n >= 232 && n <= 255); // bright black and the grey ramp
const RED_256 = (n: number) => n === 1 || n === 9 || n === 196 || n === 160 || n === 203;

/** Style in effect at the first printable character of a line (SGR sequences applied in order; reset clears). */
export function leadStyle(raw: string | undefined): { dim: boolean; red: boolean } {
  if (!raw) return { dim: false, red: false };
  let faint = false; // SGR 2 / 90: attribute, cleared only by reset
  let dimColour = false; // last foreground colour is a dim-looking one
  let red = false;
  const setColour = (dim: boolean, isRed: boolean) => { dimColour = dim; red = isRed; };
  const head = LEAD.exec(raw)?.[0] ?? "";
  for (const m of head.matchAll(SGR)) {
    const p = (m[1] === "" ? "0" : m[1]).split(";").map(Number);
    for (let i = 0; i < p.length; i++) {
      const code = p[i];
      if (code === 0) { faint = false; setColour(false, false); }
      else if (code === 2) faint = true;
      else if (code === 90) setColour(true, false);
      else if (code === 31 || code === 91) setColour(false, true);
      else if (code === 39) setColour(false, false);
      else if (code === 38 && p[i + 1] === 5) { const n = p[i + 2] ?? -1; setColour(DIM_256(n), RED_256(n)); i += 2; }
      else if (code === 38 && p[i + 1] === 2) {
        const [r, g, b] = [p[i + 2] ?? 0, p[i + 3] ?? 0, p[i + 4] ?? 0];
        setColour(isDimRgb(r, g, b), isRedRgb(r, g, b));
        i += 4;
      }
    }
  }
  return { dim: faint || dimColour, red };
}

function startKind(line: string, raw: string | undefined, isPrompt: boolean): LineKind {
  // Claude Code echoes the user's turn as "> text" (older) or "❯ text" (newer);
  // the live input line also starts with "❯", so it is excluded by index.
  if (!isPrompt && /^[>❯›]\s+\S/.test(line)) return "user";
  const agent = /^[●⏺]\s+(.*)$/.exec(line);
  if (agent) {
    const rest = agent[1] ?? "";
    if (TOOL_CALL.test(rest) || TOOL_SUMMARY.test(rest)) return null;
    const style = leadStyle(raw);
    if (style.dim || style.red) return null; // system notes and errors are not replies
    return "agent";
  }
  return null;
}

// Indented lines continue a message, except tool summaries ("Ran 2 shell
// commands"), tool results ("⎿ …") and status lines that Claude Code also indents.
const BACKGROUND = /\x1b\[(?:48;2;\d+;\d+;\d+|48;5;\d+|4[0-7]|10[0-7])m/;
/** Claude Code paints the whole user turn (every row, blank ones included) on a background band. */
export const hasBackground = (raw: string | undefined) => raw !== undefined && BACKGROUND.test(raw);

const isContinuation = (line: string) =>
  /^\s{2,}\S/.test(line) && !TOOL_SUMMARY.test(line.trim()) && !/^\s*[⎿└├│]/.test(line) && !/^\s*[※✻✽✶✳·]\s/.test(line);
const isBlank = (line: string) => line.trim() === "";

/** `raw` (with ANSI) is optional: dim-styled indented lines are tool summaries, never message text. */
export function classify(lines: string[], raw?: string[], options: { livePrompt?: boolean } = {}): LineKind[] {
  const kinds: LineKind[] = new Array(lines.length).fill(null);
  // The live prompt also starts with "❯"; it is excluded by index — unless the
  // caller already cut the input box off (`livePrompt: false`), in which case
  // the last "❯ text" line is a user turn like any other.
  const prompt = options.livePrompt === false ? -1 : promptLine(lines);
  let current: LineKind = null;
  for (let i = 0; i < lines.length; i++) {
    const line = lines[i];
    const dim = raw ? leadStyle(raw[i]).dim : false;
    const start = startKind(line, raw?.[i], i === prompt);
    if (start) {
      current = start;
      kinds[i] = start;
      continue;
    }
    if (!current) continue;
    if (current === "user" && raw && hasBackground(raw[i])) {
      kinds[i] = current; // still on the user-turn band: indentation and blanks do not matter
    } else if (current === "user" && raw && hasBackground(raw[kinds.lastIndexOf("user")])) {
      current = null; // the band ended on the previous line: this one is not part of the turn
    } else if (isContinuation(line) && !dim) {
      kinds[i] = current;
    } else if (isBlank(line) && i + 1 < lines.length && isContinuation(lines[i + 1]) && !(raw && leadStyle(raw[i + 1]).dim)) {
      kinds[i] = current; // paragraph break inside the same message
    } else {
      current = null;
    }
  }
  return kinds;
}

/** Width in cells of one code point; `next` is the following code point (for VS16). */
export type WidthOf = (ch: string, next?: string) => number;

// Compact East-Asian-Width table (wide/fullwidth → 2). Not exhaustive: the full
// transition to xterm buffer widths is M2; this covers CJK, Hangul, kana,
// fullwidth forms and the main emoji planes that Claude Code output shows.
const WIDE: [number, number][] = [
  [0x1100, 0x115f], // Hangul Jamo
  [0x2e80, 0x303e], // CJK radicals, punctuation
  [0x3041, 0x33ff], // Hiragana, Katakana, Bopomofo, compatibility
  [0x3400, 0x4dbf], // CJK Extension A
  [0x4e00, 0x9fff], // CJK Unified
  [0xa000, 0xa4cf], // Yi
  [0xac00, 0xd7a3], // Hangul syllables
  [0xf900, 0xfaff], // CJK compatibility ideographs
  [0xfe30, 0xfe4f], // CJK compatibility forms
  [0xff00, 0xff60], // fullwidth forms (halfwidth katakana FF61+ stay narrow)
  [0xffe0, 0xffe6], // fullwidth signs
  [0x1f300, 0x1faff], // emoji: misc symbols & pictographs … symbols extended-A
  [0x20000, 0x3fffd], // CJK Extension B+
];
// Emoji_Presentation=Yes code points in the dingbat/misc-symbol area
// (2600–27BF plus a few neighbours): rendered 2 cells without VS16. The rest of
// that area is text presentation (1 cell) unless followed by VS16.
const EMOJI_PRESENTATION: [number, number][] = [
  [0x231a, 0x231b], [0x23e9, 0x23ec], [0x23f0, 0x23f0], [0x23f3, 0x23f3], [0x25fd, 0x25fe],
  [0x2614, 0x2615], [0x2648, 0x2653], [0x267f, 0x267f], [0x2693, 0x2693], [0x26a1, 0x26a1],
  [0x26aa, 0x26ab], [0x26bd, 0x26be], [0x26c4, 0x26c5], [0x26ce, 0x26ce], [0x26d4, 0x26d4],
  [0x26ea, 0x26ea], [0x26f2, 0x26f3], [0x26f5, 0x26f5], [0x26fa, 0x26fa], [0x26fd, 0x26fd],
  [0x2705, 0x2705], [0x270a, 0x270b], [0x2728, 0x2728], [0x274c, 0x274c], [0x274e, 0x274e],
  [0x2753, 0x2755], [0x2757, 0x2757], [0x2795, 0x2797], [0x27b0, 0x27b0], [0x27bf, 0x27bf],
  [0x2b1b, 0x2b1c], [0x2b50, 0x2b50], [0x2b55, 0x2b55],
];
const inRanges = (cp: number, table: [number, number][]) => table.some(([lo, hi]) => cp >= lo && cp <= hi);

/**
 * Default cell width of a code point: 0 for combining marks (0300–036F),
 * variation selectors (FE0E/FE0F) and ZWJ (200D); 2 for East Asian wide /
 * fullwidth and emoji (see tables above; 2600–27BF only with emoji
 * presentation or when followed by VS16); 1 otherwise. Grapheme clusters are
 * not merged (a ZWJ sequence counts each emoji), matching xterm's cell model.
 */
export const defaultWidthOf: WidthOf = (ch, next) => {
  const cp = ch.codePointAt(0) ?? 0;
  if ((cp >= 0x0300 && cp <= 0x036f) || cp === 0xfe0e || cp === 0xfe0f || cp === 0x200d) return 0;
  if (inRanges(cp, WIDE) || inRanges(cp, EMOJI_PRESENTATION)) return 2;
  if (cp >= 0x2600 && cp <= 0x27bf && next === "️") return 2;
  return 1;
};

/** Display width of a line in cells (trailing whitespace ignored). */
function lineWidth(line: string, widthOf: WidthOf): number {
  const chars = [...line.replace(/\s+$/, "")];
  let w = 0;
  for (let i = 0; i < chars.length; i++) w += widthOf(chars[i], chars[i + 1]);
  return w;
}

/** Display width of a block: its widest line, in cells, plus padding, capped at `cols`. */
export function blockWidth(lines: string[], cols: number, widthOf: WidthOf = defaultWidthOf): number {
  let w = 0;
  for (const line of lines) w = Math.max(w, lineWidth(line, widthOf));
  return Math.min(cols, w + 2);
}

/** Drop background colours (Claude Code paints user turns on a grey band). */
export function stripBackground(raw: string): string {
  return raw.replace(/\x1b\[(?:48;2;\d+;\d+;\d+|48;5;\d+|4[0-7]|10[0-7])m/g, "");
}

/** Index of the line that carries the input prompt, if the CLI shows one. */
export function promptLine(lines: string[]): number {
  for (let i = lines.length - 1; i >= 0; i--) {
    if (/^[❯›>]\s*$/.test(lines[i]) || /^[❯›]\s/.test(lines[i])) return i;
  }
  return -1;
}

/** Widest line of a snapshot, in cells: the PTY's column count when the CLI draws full-width rules. */
export function snapshotCols(lines: string[], widthOf: WidthOf = defaultWidthOf): number {
  let w = 0;
  for (const line of lines) w = Math.max(w, lineWidth(line, widthOf));
  return w;
}

const RULE_RUN = /[─━═]{2,}/; // stateless; `new RegExp(RULE_RUN, "g")` for iteration
/**
 * A line that is only horizontal rules (Claude Code's input box, separators)
 * drawn for a wider PTY is shortened to `cols` so it never wraps in the view;
 * the longest run absorbs the cut. Other lines are returned unchanged.
 */
export function fitRules(raw: string, cols: number, widthOf: WidthOf = defaultWidthOf): string {
  const plain = stripAnsi(raw);
  if (!/^[\s─━═╌┄┈]*$/.test(plain) || !RULE_RUN.test(plain)) return raw;
  const excess = lineWidth(plain, widthOf) - cols;
  if (excess <= 0) return raw;
  let longest = 0;
  for (const m of plain.matchAll(new RegExp(RULE_RUN, "g"))) longest = Math.max(longest, m[0].length);
  let done = false;
  return raw.replace(new RegExp(RULE_RUN, "g"), (run) => {
    if (done || run.length !== longest) return run;
    done = true;
    return run.slice(0, Math.max(1, run.length - excess));
  });
}

const RULE_ONLY = /^[\s─━═╌┄┈]+$/;
/**
 * Where the CLI's input box starts: the rule line drawn just above the live
 * prompt (Claude Code boxes the prompt between two rules), else the prompt
 * line itself. -1 when there is no live prompt. Everything from here to the
 * end of the snapshot (prompt, closing rule, status line) is the input area
 * the view pins below the scrolling chat.
 */
export function inputBoxStart(lines: string[]): number {
  const prompt = promptLine(lines);
  if (prompt < 0) return -1;
  for (let i = prompt - 1; i >= Math.max(0, prompt - 2); i--) {
    if (RULE_ONLY.test(lines[i])) return i;
    if (lines[i].trim() !== "") break;
  }
  return prompt;
}

/**
 * Word-wrap one ANSI-coloured line to `maxW` cells. Escape sequences are
 * zero-width and travel with the character that follows them; the SGR state
 * in force at a break is re-emitted at the start of the continuation line,
 * which is indented like Claude Code's own continuations (2 cells). Breaks
 * happen at the last space before the limit, or mid-word when there is none.
 */
export function wrapAnsi(raw: string, maxW: number, widthOf: WidthOf = defaultWidthOf): string[] {
  if (maxW < 4) return [raw];
  const tokens: { text: string; ansi: boolean }[] = [];
  let last = 0;
  for (const m of raw.matchAll(ANSI)) {
    if (m.index! > last) tokens.push({ text: raw.slice(last, m.index), ansi: false });
    tokens.push({ text: m[0], ansi: true });
    last = m.index! + m[0].length;
  }
  if (last < raw.length) tokens.push({ text: raw.slice(last), ansi: false });
  // Flatten into per-character cells, each carrying the escapes that precede it.
  const cells: { ch: string; pre: string; w: number }[] = [];
  let pre = "";
  for (const t of tokens) {
    if (t.ansi) { pre += t.text; continue; }
    const chars = [...t.text];
    for (let i = 0; i < chars.length; i++) {
      cells.push({ ch: chars[i], pre, w: widthOf(chars[i], chars[i + 1]) });
      pre = "";
    }
  }
  if (pre) cells.push({ ch: "", pre, w: 0 });
  const indent = /^\s*/.exec(stripAnsi(raw))?.[0].length ?? 0;
  const contIndent = " ".repeat(Math.min(indent + 2, Math.max(0, maxW - 8)) || 2);
  const out: string[] = [];
  let line = "", width = 0, sgr = "", lastSpace = -1, lineStart = 0;
  const flush = (upTo: number, from: number) => {
    out.push(line.slice(0, upTo));
    line = (out.length ? contIndent : "") + sgr + line.slice(from).replace(/^\s+/, "");
    width = lineWidthOf(line, widthOf);
    lastSpace = -1;
    lineStart = out.length ? contIndent.length : 0;
  };
  for (const c of cells) {
    if (c.pre) {
      // Track the SGR state: a reset clears it, anything else accumulates.
      for (const m of c.pre.matchAll(/\x1b\[([0-9;]*)m/g)) sgr = m[1] === "" || m[1] === "0" ? "" : sgr + m[0];
    }
    // A space may overflow (trailing spaces have no width); the next
    // visible character then breaks the line at that space.
    if (width + c.w > maxW && width > 0 && c.ch !== " ") {
      if (lastSpace > lineStart) flush(lastSpace, lastSpace + 1);
      else flush(line.length, line.length);
    }
    if (c.ch === " ") lastSpace = line.length + c.pre.length;
    line += c.pre + c.ch;
    width += c.w;
  }
  out.push(line);
  return out.map((l) => l.replace(/ +$/, ""));
}

function lineWidthOf(raw: string, widthOf: WidthOf): number {
  const chars = [...stripAnsi(raw)];
  let w = 0;
  for (let i = 0; i < chars.length; i++) w += widthOf(chars[i], chars[i + 1]);
  return w;
}

/**
 * What the CLI's own input box shows right now, for Deck's chat input:
 * `suggestion` — text in the prompt (Claude Code's ghost suggestion, taken
 * with Tab; Deck never types there itself, so anything but `[Image #N]`
 * placeholders is a suggestion) and `status` — the line under the box
 * ("⏵⏵ bypass permissions on (shift+tab to cycle)", "? for shortcuts").
 * Both empty when there is no live prompt.
 */
export interface InputHint {
  suggestion: string;
  status: string;
  /** The rest of the CLI's bottom panel after the status line: background agents, monitors, shells. */
  below: string[];
  /** The CLI's own activity line above the box (`✻ Waiting for 2 background agents to finish`, `✻ Cooked for 42s`). */
  activity: string;
}
const NONE: InputHint = { suggestion: "", status: "", below: [], activity: "" };
export function inputHint(raw: string): InputHint {
  const lines = raw.split("\n").map((l) => stripAnsi(l).replace(/\s+$/, ""));
  const prompt = promptLine(lines);
  if (prompt < 0) return NONE;
  let text = lines[prompt].replace(/^[❯›>]\s?/, "");
  // `❯ 1. Yes` is a menu cursor (permission prompt, AskUserQuestion), not the prompt.
  if (/^\d+\.\s/.test(text)) return NONE;
  let activity = "";
  for (let j = inputBoxStart(lines) - 1; j >= 0 && j >= prompt - 12; j--) {
    const t = lines[j].trim();
    if (/^[✻✳✶✽✢·*]\s/.test(t)) { activity = t.replace(/^[✻✳✶✽✢·*]\s*/, ""); break; }
    if (t) break;
  }
  // continuation lines of a multi-line prompt, up to the closing rule
  for (let i = prompt + 1; i < lines.length && !RULE_ONLY.test(lines[i]) && lines[i].trim() !== ""; i++) text += " " + lines[i].trim();
  text = text.replace(/\s+/g, " ").trim();
  if (/^(\[Image #\d+\]\s*)*$/.test(text)) text = "";
  let status = "";
  const below: string[] = [];
  let i = prompt + 1;
  while (i < lines.length && !RULE_ONLY.test(lines[i])) i++;
  for (i += 1; i < lines.length; i++) {
    const l = lines[i].trim().replace(/\s{2,}/g, " · ");
    if (!l) continue;
    if (!status) status = l; else below.push(l);
  }
  return { suggestion: text, status, below, activity };
}

export interface CliMenu {
  /** Text above the options (the question / what is being asked). */
  title: string;
  options: { n: number; label: string }[];
  /** Index (0-based) of the option under the `❯` cursor. */
  current: number;
}

const BOX = /[│┃╭╮╰╯┌┐└┘├┤]/g; // verticals and corners; rules stay (they separate options)
const OPTION = /^\s*(❯\s*)?(\d+)\.\s+(.*)$/;
/**
 * The CLI's numbered menu on screen (a permission prompt, an AskUserQuestion,
 * a model picker…): the last run of `N. label` lines, one of them under the
 * `❯` cursor, with the text above it as the title. Null when there is none.
 */
export function menuOf(raw: string): CliMenu | null {
  const lines = raw.split("\n").map((l) => stripAnsi(l).replace(BOX, " ").replace(/\s+$/, ""));
  let last = -1;
  for (let i = lines.length - 1; i >= 0; i--) if (OPTION.test(lines[i])) { last = i; break; }
  if (last < 0) return null;
  // A live menu sits at the bottom of the screen: after its last option only a
  // footer line or two may follow. A numbered list higher up is history (a
  // user prompt echoed with `❯`, an agent's list) — not something to click.
  const tail = lines.slice(last + 1).filter((l) => l.trim() !== "");
  if (tail.length > 3) return null;
  // walk up through options and their wrapped continuation lines
  const options: { n: number; label: string; line: number; cur: boolean }[] = [];
  let i = last;
  let first = last;
  while (i >= 0) {
    const m = OPTION.exec(lines[i]);
    if (m) {
      options.unshift({ n: Number(m[2]), label: m[3].trim(), line: i, cur: !!m[1] });
      first = i;
      i--;
      continue;
    }
    // a continuation line belongs to the option above it (indented, no number);
    // a rule between options (Claude Code separates "Chat about this") is skipped
    if (options.length && lines[i].trim() !== "" && /^\s{3,}/.test(lines[i]) && i < last) { i--; continue; }
    if (options.length && RULE_ONLY.test(lines[i])) { i--; continue; }
    break;
  }
  if (!options.length) return null;
  // continuation lines below each option (wrapped descriptions) join its label
  for (let k = 0; k < options.length; k++) {
    const end = k + 1 < options.length ? options[k + 1].line : last + 1;
    for (let j = options[k].line + 1; j < end; j++) if (!RULE_ONLY.test(lines[j])) options[k].label += " " + lines[j].trim();
    options[k].label = options[k].label.replace(/\s+/g, " ").trim();
  }
  const cur = options.findIndex((o) => o.cur);
  if (cur < 0) return null; // numbered text, but no cursor: not a live menu
  // numbering must be 1..N in order
  if (options.some((o, k) => o.n !== k + 1)) return null;
  const title: string[] = [];
  for (let j = first - 1; j >= 0 && title.length < 8; j--) {
    const t = lines[j].trim();
    if (!t) { if (title.length) break; continue; }
    if (RULE_ONLY.test(t)) break;
    title.unshift(t);
  }
  return { title: title.join(" ").replace(/\s+/g, " ").trim(), options: options.map((o) => ({ n: o.n, label: o.label })), current: cur };
}

/** Claude Code's permission mode as its status line names it (Shift+Tab cycles). */
export type CliMode = "default" | "accept-edits" | "plan" | "bypass";
export const MODE_LABEL: Record<CliMode, string> = { default: "DEFAULT", "accept-edits": "ACCEPT EDITS", plan: "PLAN", bypass: "BYPASS" };

/** `{ mode, rest }`: the mode read from the status line, and the line without the mode phrase and its hint. */
export function cliMode(status: string): { mode: CliMode; rest: string } {
  const m = /(bypass permissions on|accept edits on|plan mode on)/i.exec(status);
  const mode: CliMode = !m ? "default" : /bypass/i.test(m[1]) ? "bypass" : /accept/i.test(m[1]) ? "accept-edits" : "plan";
  const rest = status
    .replace(/[⏵▸▶]+\s*/g, "")
    .replace(/(bypass permissions on|accept edits on|plan mode on)/i, "")
    .replace(/\(shift\+tab to cycle\)/i, "")
    .replace(/shift\+tab to cycle/i, "")
    .replace(/^[\s·]+|[\s·]+$/g, "")
    .replace(/\s*·\s*·\s*/g, " · ")
    .trim();
  return { mode, rest };
}
