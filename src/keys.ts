/**
 * xterm.js `onData` → Herdr input (docs/architecture.md F5/F6).
 *
 * `pane.send_text` wraps its argument in bracketed paste when the pane enabled
 * it (F5), so escape sequences and control bytes must never travel as text:
 * they go through `pane.send_keys` as key names Herdr's `config::parse_key_combo`
 * accepts (F6; fork `src/config/keybinds.rs`), or they are dropped.
 *
 * Home/End/Insert/Delete/PageUp/PageDown are named `home end insert delete
 * pageup pagedown` — names the fork's `parse_key_combo` accepts since M1.5
 * (upstream Herdr still answers `invalid_key` for them).
 */
export type MappedInput = { keys: string[] } | { text: string } | { drop: true; reason: string };

const ESC = "\x1b";

// Sequences that map to a bare key name (no modifier).
const PLAIN: Record<string, string> = {
  "\r": "enter",
  "\x7f": "backspace",
  "\t": "tab",
  [ESC]: "esc",
  "\x1b[Z": "shift+tab",
  "\x1b[A": "up", "\x1b[B": "down", "\x1b[C": "right", "\x1b[D": "left",
  "\x1bOA": "up", "\x1bOB": "down", "\x1bOC": "right", "\x1bOD": "left",
  "\x1bOP": "f1", "\x1bOQ": "f2", "\x1bOR": "f3", "\x1bOS": "f4",
  "\x1b[11~": "f1", "\x1b[12~": "f2", "\x1b[13~": "f3", "\x1b[14~": "f4",
  "\x1b[15~": "f5", "\x1b[17~": "f6", "\x1b[18~": "f7", "\x1b[19~": "f8",
  "\x1b[20~": "f9", "\x1b[21~": "f10", "\x1b[23~": "f11", "\x1b[24~": "f12",
};

// Keys xterm sends that Herdr's key grammar cannot name (fork keybinds.rs:1313-1343).
// Navigation keys. Herdr's `parse_key_combo` learned these names in the fork
// (M1.5); upstream answers `invalid_key` and the key is silently lost there.
const NAMED_NAV: Record<string, string> = {
  "\x1b[H": "home", "\x1b[1~": "home", "\x1bOH": "home",
  "\x1b[F": "end", "\x1b[4~": "end", "\x1bOF": "end",
  "\x1b[2~": "insert", "\x1b[3~": "delete", "\x1b[5~": "pageup", "\x1b[6~": "pagedown",
};

// Final byte of `CSI 1;N <final>` → key; number of `CSI <n>;N ~` → key.
const CSI_FINAL: Record<string, string> = {
  A: "up", B: "down", C: "right", D: "left", H: "home", F: "end", P: "f1", Q: "f2", R: "f3", S: "f4",
};
const CSI_TILDE: Record<string, string> = {
  "1": "home", "2": "insert", "3": "delete", "4": "end", "5": "pageup", "6": "pagedown",
  "11": "f1", "12": "f2", "13": "f3", "14": "f4", "15": "f5", "17": "f6", "18": "f7", "19": "f8",
  "20": "f9", "21": "f10", "23": "f11", "24": "f12",
};

// C0 bytes with a name other than ctrl+<letter>.
const C0_SPECIAL: Record<number, string> = {
  0x00: "ctrl+@", 0x1c: "ctrl+\\", 0x1d: "ctrl+]", 0x1e: "ctrl+^", 0x1f: "ctrl+_",
};

/** xterm modifier parameter (2..8) → prefix in Herdr spelling, or null. */
function modifierPrefix(n: number): string | null {
  if (!Number.isInteger(n) || n < 2 || n > 8) return null;
  const bits = n - 1; // 1 = shift, 2 = alt, 4 = ctrl
  const parts: string[] = [];
  if (bits & 4) parts.push("ctrl");
  if (bits & 2) parts.push("alt");
  if (bits & 1) parts.push("shift");
  return parts.join("+");
}

const drop = (reason: string): MappedInput => ({ drop: true, reason });
const key = (name: string): MappedInput => ({ keys: [name] });

/** Name for a single key sequence (no leading alt-ESC). Null if unknown. */
function nameOf(data: string): string | null {
  if (data in PLAIN) return PLAIN[data];
  if (data in NAMED_NAV) return NAMED_NAV[data];
  if (data.length === 1) {
    const c = data.charCodeAt(0);
    if (c in C0_SPECIAL) return C0_SPECIAL[c];
    if (c >= 0x01 && c <= 0x1a) return `ctrl+${String.fromCharCode(96 + c)}`;
    return null;
  }
  let m = /^\x1b\[1;(\d+)([A-DHFPQRS])$/.exec(data);
  if (m) {
    const prefix = modifierPrefix(Number(m[1]));
    return prefix ? `${prefix}+${CSI_FINAL[m[2]]}` : null;
  }
  m = /^\x1b\[(\d+);(\d+)~$/.exec(data);
  if (m) {
    const base = CSI_TILDE[m[1]];
    const prefix = modifierPrefix(Number(m[2]));
    return base && prefix ? `${prefix}+${base}` : null;
  }
  return null;
}

function isPrintable(ch: string): boolean {
  const c = ch.codePointAt(0)!;
  return c >= 0x20 && c !== 0x7f && !(c >= 0x80 && c < 0xa0);
}

export function mapInput(data: string): MappedInput {
  if (data === "") return drop("empty input");
  const hasEsc = data.includes(ESC);
  // Control bytes other than \t \n \r, which are legitimate paste content.
  const hasControl = /[\x00-\x08\x0b\x0c\x0e-\x1f\x7f]/.test(data);

  if (!hasEsc && !hasControl) {
    // A lone \r / \t / \n is a key press (enter, tab, ctrl+j); inside a longer
    // paste the same bytes are content.
    if (data === "\r" || data === "\t" || data === "\n") return key(nameOf(data)!);
    return { text: data };
  }

  if (!hasEsc) {
    if (data.length === 1) {
      const name = nameOf(data);
      return name ? key(name) : drop(`unmapped control byte 0x${data.charCodeAt(0).toString(16)}`);
    }
    return drop(`control bytes in text ${JSON.stringify(data)}`);
  }

  const name = nameOf(data);
  if (name) return key(name);

  // alt+<key>: ESC followed by exactly one key (a printable char or a known sequence).
  if (data.startsWith(ESC)) {
    const rest = data.slice(1);
    const restChars = [...rest];
    if (restChars.length === 1 && isPrintable(rest)) {
      const ch = restChars[0];
      return { keys: [`alt+${ch === " " ? "space" : ch === "+" ? "plus" : ch}`] };
    }
    const inner = rest === "" ? null : nameOf(rest);
    if (inner && !inner.includes("+")) return key(`alt+${inner}`);
  }
  return drop(`unknown escape sequence ${JSON.stringify(data)}`);
}
