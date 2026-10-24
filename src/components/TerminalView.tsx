import { createEffect, createSignal, on, onCleanup, onMount } from "solid-js";
import { Terminal, type IDecoration } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { api, type Tree } from "../api";
import { blockWidth, classify, promptLine, stripAnsi, stripBackground, type LineKind } from "../dialogue";

const COLOR: Record<Exclude<LineKind, null>, string> = {
  user: "\x1b[38;2;255;246;168m",
  agent: "\x1b[38;2;143;245;184m",
};

/**
 * Milestone 1 pane view: an xterm that shows Herdr's ANSI snapshot of the pane
 * (`pane.read`) and forwards keystrokes with `pane.send_text` / `pane.send_keys`.
 * Refreshes on `pane.updated` events (via `tick`) with a short debounce, plus a
 * slow fallback poll. Dialogue blocks are recoloured and framed with xterm
 * decorations; the cursor is placed on the CLI's prompt line.
 */
export default function TerminalView(p: { paneId: string; tick: number; tree?: Tree }) {
  let host!: HTMLDivElement;
  let term: Terminal;
  let fit: FitAddon;
  let lastText = "";
  let debounce: number | undefined;
  let decorations: IDecoration[] = [];

  const pane = () => p.tree?.panes.find((x) => x.pane_id === p.paneId);
  const header = () => {
    const x = pane();
    if (!x) return { title: p.paneId, where: "", ctx: "" };
    const who = x.agent ? `${(x.display_agent ?? x.agent).toUpperCase()} · ${x.label ?? x.title ?? ""}` : (x.title ?? "SHELL");
    const ws = p.tree?.workspaces.find((w) => w.workspace_id === x.workspace_id)?.label ?? x.workspace_id;
    return { title: who, where: `${ws} · ${x.cwd ?? ""}`, ctx: x.tokens?.ctx ? `CTX ${x.tokens.ctx}%` : "" };
  };

  const [error, setError] = createSignal<string | null>(null);
  async function refresh() {
    try {
      const { read } = await api.paneRead(p.paneId, Math.max(200, term.rows * 4));
      setError(null);
      api.debug(`read pane=${p.paneId} len=${read.text.length} rev=${read.revision} rows=${term.rows} cols=${term.cols} same=${read.text === lastText} head=${JSON.stringify(read.text.slice(0, 120))}`);
      if (read.text === lastText) return;
      lastText = read.text;
      render(read.text);
    } catch (err) {
      console.error("pane.read failed", err);
      setError(String(err));
    }
  }

  function render(text: string) {
    const raw = text.replace(/\r\n?/g, "\n").split("\n");
    const rawPlain = raw.map(stripAnsi);
    const rawKinds = classify(rawPlain, raw);
    // Every dialogue block gets its own blank row inside the frame at the top
    // and bottom; blank lines around it in the output stay outside as the gap.
    const lines: string[] = [], plain: string[] = [], kinds: LineKind[] = [];
    const push = (l: string, pl: string, k: LineKind) => { lines.push(l); plain.push(pl); kinds.push(k); };
    let i = 0;
    while (i < raw.length) {
      const k = rawKinds[i];
      if (!k) { push(raw[i], rawPlain[i], null); i++; continue; }
      let j = i;
      while (j + 1 < raw.length && rawKinds[j + 1] === k) j++;
      push("", "", k);
      for (let r = i; r <= j; r++) push(raw[r], rawPlain[r], k);
      push("", "", k);
      i = j + 1;
    }
    const out = lines
      .map((line, i) => (kinds[i] ? `${COLOR[kinds[i]!]}${kinds[i] === "user" ? stripBackground(line) : line}\x1b[39m` : line))
      .join("\r\n");
    for (const d of decorations) d.dispose();
    decorations = [];
    term.reset();
    api.debug(`render lines=${lines.length} blocks=${kinds.filter((k, i) => k && kinds[i - 1] !== k).length} out=${out.length} host=${host.clientWidth}x${host.clientHeight}`);
    term.write(out, () => {
      try {
        api.debug(`written buffer=${term.buffer.active.length} baseY=${term.buffer.active.baseY} line0=${JSON.stringify(term.buffer.active.getLine(0)?.translateToString(true).slice(0, 80))}`);
        decorate(kinds, plain);
        placeCursor(plain);
      } catch (err) {
        console.error("decorate failed", err);
        setError(String(err));
      }
    });
  }

  // Logical line → buffer rows (long lines wrap in xterm).
  function rowsByLine(): number[][] {
    const buf = term.buffer.active;
    const map: number[][] = [];
    for (let r = 0; r < buf.length; r++) {
      const line = buf.getLine(r);
      if (!line) break;
      if (line.isWrapped && map.length) map[map.length - 1].push(r);
      else map.push([r]);
    }
    return map;
  }

  function decorate(kinds: LineKind[], plain: string[]) {
    const rows = rowsByLine();
    const buf = term.buffer.active;
    const cursorRow = buf.baseY + buf.cursorY;
    let i = 0;
    while (i < kinds.length) {
      const kind = kinds[i];
      if (!kind) { i++; continue; }
      let j = i;
      while (j + 1 < kinds.length && kinds[j + 1] === kind) j++;
      const first = rows[i]?.[0];
      const last = rows[j]?.[rows[j].length - 1];
      if (first !== undefined && last !== undefined) {
        const width = blockWidth(plain.slice(i, j + 1), term.cols);
        for (let r = first; r <= last; r++) {
          const marker = term.registerMarker(r - cursorRow);
          if (!marker) continue;
          const deco = term.registerDecoration({ marker, x: 0, width, layer: "bottom" });
          if (!deco) continue;
          const pos = r === first ? (r === last ? "only" : "top") : r === last ? "bottom" : "mid";
          deco.onRender((el) => {
            el.classList.add("dlg", `dlg-${kind}`, `dlg-${pos}`);
            if (pos === "top" || pos === "only") el.dataset.who = kind === "user" ? "YOU" : (pane()?.display_agent ?? pane()?.agent ?? "AGENT").toUpperCase();
          });
          decorations.push(deco);
        }
      }
      i = j + 1;
    }
  }

  function placeCursor(plain: string[]) {
    const idx = promptLine(plain);
    const rows = rowsByLine();
    const row = idx >= 0 ? rows[idx]?.[0] : undefined;
    if (row === undefined) { term.options.cursorStyle = "underline"; term.options.cursorInactiveStyle = "none"; return; }
    const buf = term.buffer.active;
    const viewportRow = row - buf.baseY;
    if (viewportRow < 0 || viewportRow >= term.rows) return;
    // The snapshot carries no cursor position; typing appends, so the end of the
    // prompt line is the best estimate until the streaming attach lands.
    const typed = [...plain[idx].replace(/\s+$/, "")].length;
    const col = Math.min(term.cols - 1, Math.max(typed, 2));
    term.options.cursorStyle = "block";
    term.options.cursorInactiveStyle = "outline";
    term.write(`\x1b[${viewportRow + 1};${col + 1}H`);
  }

  const schedule = () => { clearTimeout(debounce); debounce = window.setTimeout(refresh, 120); };

  // Printable text goes as text; control sequences map to Herdr key names.
  const KEYS: Record<string, string> = {
    "\r": "enter", "\x7f": "backspace", "\t": "tab", "\x1b": "esc",
    "\x1b[A": "up", "\x1b[B": "down", "\x1b[C": "right", "\x1b[D": "left",
    "\x1b[H": "home", "\x1b[F": "end", "\x1b[5~": "pageup", "\x1b[6~": "pagedown", "\x1b[3~": "delete",
  };
  function onData(data: string) {
    const key = KEYS[data];
    if (key) return void api.sendKeys(p.paneId, [key]).then(schedule);
    if (data.length === 1 && data.charCodeAt(0) < 32) {
      const letter = String.fromCharCode(data.charCodeAt(0) + 96);
      return void api.sendKeys(p.paneId, [`ctrl+${letter}`]).then(schedule);
    }
    api.sendText(p.paneId, data).then(schedule);
  }

  onMount(() => {
    term = new Terminal({
      fontFamily: '"Cascadia Mono", Consolas, "JetBrains Mono", monospace',
      fontSize: 13.5,
      lineHeight: 1.25,
      cursorBlink: true,
      cursorStyle: "block",
      scrollback: 5000,
      allowProposedApi: true,
      theme: {
        background: "#080406", foreground: "#cfe9ea", cursor: "#67E0E3", cursorAccent: "#120A10",
        selectionBackground: "#143A3C",
        black: "#120A10", red: "#FF2B4A", green: "#3FA7AA", yellow: "#FCEE0A", blue: "#67E0E3",
        magenta: "#C0132A", cyan: "#67E0E3", white: "#9BE8EA",
        brightBlack: "#7A5C6E", brightRed: "#FF6B7A", brightGreen: "#2BFF88", brightYellow: "#FFF6A8",
        brightBlue: "#9BE8EA", brightMagenta: "#FF2B4A", brightCyan: "#9BE8EA", brightWhite: "#FFFFFF",
      },
    });
    fit = new FitAddon();
    term.loadAddon(fit);
    term.open(host);
    fit.fit();
    term.onData(onData);
    const ro = new ResizeObserver(() => { fit.fit(); if (lastText) render(lastText); });
    ro.observe(host);
    const poll = setInterval(refresh, 2500);
    onCleanup(() => { ro.disconnect(); clearInterval(poll); term.dispose(); });
    refresh();
  });

  createEffect(on(() => p.paneId, () => { lastText = ""; refresh(); }, { defer: true }));
  createEffect(on(() => p.tick, schedule, { defer: true }));

  return (
    <div class="term">
      <div class="th"><span>◆ {header().title}</span><span class="micro">{header().where}</span><span class="micro right" style={error() ? "color:#FF2B4A" : ""}>{error() ?? header().ctx}</span></div>
      <div class="xterm-host" ref={host} />
    </div>
  );
}
