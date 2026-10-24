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
    const dim = raw ? leadStyle(raw[i]).dim : false;
    const start = startKind(line, raw?.[i], i === prompt);
    if (start) {
      current = start;
      kinds[i] = start;
      continue;
    }
    if (!current) continue;
    if (isContinuation(line) && !dim) {
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
