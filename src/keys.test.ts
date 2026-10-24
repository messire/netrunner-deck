import { describe, expect, it } from "vitest";
import { mapInput } from "./keys";

const keys = (data: string) => {
  const r = mapInput(data);
  expect(r, `mapInput(${JSON.stringify(data)})`).toHaveProperty("keys");
  return (r as { keys: string[] }).keys;
};
const text = (data: string) => {
  const r = mapInput(data);
  expect(r, `mapInput(${JSON.stringify(data)})`).toHaveProperty("text");
  return (r as { text: string }).text;
};
const dropped = (data: string) => {
  const r = mapInput(data);
  expect(r, `mapInput(${JSON.stringify(data)})`).toMatchObject({ drop: true });
  expect(r).not.toHaveProperty("text");
  expect(r).not.toHaveProperty("keys");
};

describe("mapInput: printable text goes through pane.send_text", () => {
  it.each(["a", "Z", " ", "~", "ё", "日本", "🙂", "abc", "hello world"])("%j → text", (s) => {
    expect(text(s)).toBe(s);
  });
  it("multi-line paste without ESC stays text (newlines/tabs are paste content)", () => {
    expect(text("line1\rline2")).toBe("line1\rline2");
    expect(text("a\tb\nc")).toBe("a\tb\nc");
  });
  it("a paste that contains ESC is never sent as text (F5: it would arrive as a bracketed paste)", () => {
    dropped("abc\x1b[31mred");
    dropped("\x1b[200~pasted\x1b[201~");
  });
  it("a paste with other control bytes is dropped rather than pasted", () => {
    dropped("ab\x07c");
    dropped("ab\x7fc");
  });
  it("empty input is dropped", () => {
    dropped("");
  });
});

describe("mapInput: single control keys", () => {
  it.each([
    ["\r", "enter"],
    ["\x7f", "backspace"],
    ["\t", "tab"],
    ["\x1b", "esc"],
    ["\x1b[Z", "shift+tab"],
  ])("%j → %s", (data, name) => {
    expect(keys(data)).toEqual([name]);
  });
});

describe("mapInput: ctrl+letter and the C0 specials", () => {
  it("0x01–0x1a → ctrl+a..ctrl+z (except the bytes that are their own keys)", () => {
    for (let c = 1; c <= 26; c++) {
      const data = String.fromCharCode(c);
      if (data === "\t" || data === "\r") continue; // tab / enter, tested above
      expect(keys(data)).toEqual([`ctrl+${String.fromCharCode(96 + c)}`]);
    }
  });
  it("\\x01 → ctrl+a, \\x03 → ctrl+c, \\x0a → ctrl+j", () => {
    expect(keys("\x01")).toEqual(["ctrl+a"]);
    expect(keys("\x03")).toEqual(["ctrl+c"]);
    expect(keys("\n")).toEqual(["ctrl+j"]);
  });
  it.each([
    ["\x00", "ctrl+@"],
    ["\x1c", "ctrl+\\"],
    ["\x1d", "ctrl+]"],
    ["\x1e", "ctrl+^"],
    ["\x1f", "ctrl+_"],
  ])("%j → %s", (data, name) => {
    expect(keys(data)).toEqual([name]);
  });
});

describe("mapInput: cursor and function keys", () => {
  it.each([
    ["\x1b[A", "up"], ["\x1b[B", "down"], ["\x1b[C", "right"], ["\x1b[D", "left"],
    ["\x1bOA", "up"], ["\x1bOB", "down"], ["\x1bOC", "right"], ["\x1bOD", "left"],
  ])("%j → %s", (data, name) => {
    expect(keys(data)).toEqual([name]);
  });
  it.each([
    ["\x1bOP", "f1"], ["\x1bOQ", "f2"], ["\x1bOR", "f3"], ["\x1bOS", "f4"],
    ["\x1b[11~", "f1"], ["\x1b[12~", "f2"], ["\x1b[13~", "f3"], ["\x1b[14~", "f4"],
    ["\x1b[15~", "f5"], ["\x1b[17~", "f6"], ["\x1b[18~", "f7"], ["\x1b[19~", "f8"],
    ["\x1b[20~", "f9"], ["\x1b[21~", "f10"], ["\x1b[23~", "f11"], ["\x1b[24~", "f12"],
  ])("%j → %s", (data, name) => {
    expect(keys(data)).toEqual([name]);
  });
  it("\\x1b[16~ and \\x1b[22~ are gaps in the F-key numbering → dropped", () => {
    dropped("\x1b[16~");
    dropped("\x1b[22~");
  });
});

describe("mapInput: home/end/insert/delete/pageup/pagedown", () => {
  // Herdr's `config::parse_key_combo` (src/config/keybinds.rs) has no token for
  // these keys, so `pane.send_keys` answers `invalid_key: unsupported key home`.
  // Until the fork learns them they are recognised and dropped (F5 forbids
  // sending the escape sequence as text).
  it.each([
    "\x1b[H", "\x1b[F", "\x1b[1~", "\x1b[4~", "\x1bOH", "\x1bOF",
    "\x1b[2~", "\x1b[3~", "\x1b[5~", "\x1b[6~",
    "\x1b[1;5H", "\x1b[1;2F", "\x1b[3;5~", "\x1b[5;2~",
  ])("%j → drop (no Herdr key name yet)", (data) => {
    dropped(data);
  });
});

describe("mapInput: modified keys (CSI 1;N x / CSI n;N ~)", () => {
  it.each([
    ["\x1b[1;2A", "shift+up"],
    ["\x1b[1;3B", "alt+down"],
    ["\x1b[1;4C", "alt+shift+right"],
    ["\x1b[1;5C", "ctrl+right"],
    ["\x1b[1;5D", "ctrl+left"],
    ["\x1b[1;6D", "ctrl+shift+left"],
    ["\x1b[1;7A", "ctrl+alt+up"],
    ["\x1b[1;8A", "ctrl+alt+shift+up"],
    ["\x1b[1;5P", "ctrl+f1"],
    ["\x1b[1;2S", "shift+f4"],
    ["\x1b[15;5~", "ctrl+f5"],
    ["\x1b[24;2~", "shift+f12"],
    ["\x1b[17;3~", "alt+f6"],
  ])("%j → %s", (data, name) => {
    expect(keys(data)).toEqual([name]);
  });
  it("modifier numbers outside 2..8 are dropped, not guessed", () => {
    dropped("\x1b[1;1A");
    dropped("\x1b[1;9A");
    dropped("\x1b[1;0C");
    dropped("\x1b[15;12~");
  });
});

describe("mapInput: alt-prefixed keys (ESC + key)", () => {
  it.each([
    ["\x1ba", "alt+a"], ["\x1bz", "alt+z"], ["\x1b1", "alt+1"], ["\x1b.", "alt+."],
    ["\x1bA", "alt+A"],
    ["\x1b ", "alt+space"],
    ["\x1b+", "alt+plus"],
    // xterm delivers one complete sequence per keystroke, so a bare ESC + `[`
    // or ESC + `O` is Alt+[ / Alt+Shift+o, not a truncated CSI/SS3 prefix.
    ["\x1b[", "alt+["],
    ["\x1bO", "alt+O"],
    ["\x1b\r", "alt+enter"],
    ["\x1b\x7f", "alt+backspace"],
    ["\x1b\x1b[A", "alt+up"],
    ["\x1b\x1b[D", "alt+left"],
  ])("%j → %s", (data, name) => {
    expect(keys(data)).toEqual([name]);
  });
  it("ESC + control byte or ESC + multi-char text is not alt+key → dropped", () => {
    dropped("\x1b\x01");
    dropped("\x1babc");
  });
});

describe("mapInput: unknown escape sequences are dropped, never pasted (F5)", () => {
  it.each(["\x1b[99~", "\x1b[?25h", "\x1b]0;title\x07", "\x1b[1;5", "\x1b[M abc", "\x1b[<0;10;10M"])("%j → drop", (data) => {
    dropped(data);
  });
  it("a drop carries a reason string for console.debug", () => {
    const r = mapInput("\x1b[99~");
    expect(r).toMatchObject({ drop: true });
    expect(typeof (r as { reason: unknown }).reason).toBe("string");
  });
});

describe("mapInput: every emitted key name is accepted by Herdr's parse_key_combo", () => {
  // Mirror of `parse_key_combo` / `parse_modifier_token` in the Herdr fork,
  // src/config/keybinds.rs (read 2026-10-24, lines 1231-1343): a combo is
  // `+`-separated tokens, every token but one a modifier, the remaining token a
  // key name below, a single character, or `f<N>`. A literal `+` key is
  // spelled `plus` (or a trailing `++`). Empty tokens reject the whole combo,
  // so `alt+ ` would be refused — `alt+space` is the accepted spelling.
  const MODIFIERS = new Set(["ctrl", "control", "shift", "alt", "option", "meta", "cmd", "command", "super", "hyper"]);
  const KEY_NAMES = new Set([
    "space", "enter", "return", "esc", "escape", "tab", "backspace", "bs",
    "left", "right", "up", "down",
    "minus", "comma", "period", "slash", "backslash", "quote", "double_quote", "double-quote",
    "semicolon", "colon", "percent", "ampersand", "backtick", "plus",
  ]);
  function herdrAccepts(combo: string): boolean {
    if (combo !== combo.trim()) return false;
    if (combo === "+") return true;
    if (combo.endsWith("++")) {
      const mods = combo.slice(0, -2).split("+");
      return mods.length > 0 && mods.every((m) => MODIFIERS.has(m.toLowerCase()));
    }
    const parts = combo.split("+");
    let key: string | null = null;
    for (const part of parts) {
      if (part === "") return false;
      if (MODIFIERS.has(part.toLowerCase())) continue;
      if (key !== null) return false;
      key = part;
    }
    if (key === null) return false;
    if (KEY_NAMES.has(key.toLowerCase())) return true;
    if ([...key].length === 1) return true;
    return /^f\d+$/i.test(key);
  }

  // Everything the mapper can be fed that yields keys: all single bytes, every
  // known sequence, every modifier variant of every CSI sequence, alt + each.
  function allInputs(): string[] {
    const out: string[] = [];
    for (let c = 0; c < 128; c++) out.push(String.fromCharCode(c));
    const finals = ["A", "B", "C", "D", "H", "F", "P", "Q", "R", "S", "Z"];
    const tildes = [1, 2, 3, 4, 5, 6, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 99];
    for (const f of finals) { out.push(`\x1b[${f}`); out.push(`\x1bO${f}`); }
    for (const n of tildes) out.push(`\x1b[${n}~`);
    for (let m = 0; m <= 9; m++) {
      for (const f of finals) out.push(`\x1b[1;${m}${f}`);
      for (const n of tildes) out.push(`\x1b[${n};${m}~`);
    }
    const plain = [...out];
    for (const s of plain) out.push(`\x1b${s}`);
    return out;
  }

  it("herdrAccepts mirror sanity-checks against known good/bad spellings", () => {
    for (const ok of ["enter", "shift+tab", "ctrl+shift+left", "alt+plus", "alt++", "ctrl+\\", "f12", "ctrl+@", "alt+space", "alt+A"]) expect(herdrAccepts(ok), ok).toBe(true);
    for (const bad of ["home", "end", "pageup", "pagedown", "insert", "delete", "alt+ ", "ctrl+", "ctrl+a+b", "", "alt+pgup"]) expect(herdrAccepts(bad), bad).toBe(false);
  });

  it("no key name leaves mapInput that Herdr would reject with invalid_key", () => {
    const seen = new Map<string, string>();
    for (const data of allInputs()) {
      const r = mapInput(data);
      if (!("keys" in r)) continue;
      expect(r.keys.length, JSON.stringify(data)).toBeGreaterThan(0);
      for (const k of r.keys) seen.set(k, data);
    }
    const rejected = [...seen].filter(([k]) => !herdrAccepts(k)).map(([k, d]) => `${JSON.stringify(d)} → ${k}`);
    expect(rejected).toEqual([]);
    // Guard against the loop silently covering nothing.
    expect(seen.size).toBeGreaterThan(60);
  });
});
