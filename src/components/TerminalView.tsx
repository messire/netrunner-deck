import { createEffect, createSignal, on, onCleanup, onMount } from "solid-js";
import { Terminal, type IDecoration } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { api, type Tree } from "../api";
import { blockWidth, classify, fitRules, promptLine, stripAnsi, stripBackground, type LineKind } from "../dialogue";
import { mapInput } from "../keys";

const COLOR: Record<Exclude<LineKind, null>, string> = {
  user: "\x1b[38;2;255;246;168m",
  agent: "\x1b[38;2;143;245;184m",
};

/**
 * Milestone 1 pane view: an xterm that shows Herdr's ANSI snapshot of the pane
 * (`pane.read`) and forwards keystrokes with `pane.send_text` / `pane.send_keys`
 * (mapping in `src/keys.ts`). The snapshot is refreshed shortly after own
 * input, on the fork's `pane_output_changed` tick for this pane (M1.5), and by
 * a poll: 2.5 s until the first tick proves the server emits output events,
 * then a 10 s safety net (architecture.md F3).
 * Dialogue blocks are recoloured and framed with xterm decorations; the cursor
 * is placed on the CLI's prompt line.
 */
export const POLL_MS_WITHOUT_OUTPUT_EVENTS = 2500;
export const POLL_MS_WITH_OUTPUT_EVENTS = 10000;

export default function TerminalView(p: { paneId: string; tree?: Tree; outputTick?: number }) {
  let host!: HTMLDivElement;
  let term: Terminal;
  let fit: FitAddon;
  let lastText = "";
  // `revision` from the last applied pane.read (fork: content counter; upstream: always 0).
  let lastRevision = 0;
  // Set by the first output tick: the server emits output events, so the poll
  // may relax from 2.5 s to a 10 s safety net.
  let seenTick = false;
  let debounce: number | undefined;
  let decorations: IDecoration[] = [];
  // Set on dispose: pending timers/promises must not touch the disposed xterm or signals.
  let disposed = false;
  // "Last pane.read wins" per pane id: a slow response for an older request is dropped.
  onCleanup(() => { disposed = true; clearTimeout(debounce); });

  const pane = () => p.tree?.panes.find((x) => x.pane_id === p.paneId);
  const header = () => {
    const x = pane();
    if (!x) return { title: p.paneId, where: "", ctx: "" };
    const who = x.agent ? `${(x.display_agent ?? x.agent).toUpperCase()} · ${x.label ?? x.title ?? ""}` : (x.title ?? "SHELL");
    const ws = p.tree?.workspaces.find((w) => w.workspace_id === x.workspace_id)?.label ?? x.workspace_id;
    return { title: who, where: `${ws} · ${x.cwd ?? ""}`, ctx: x.tokens?.ctx ? `CTX ${x.tokens.ctx}%` : "" };
  };

  const [error, setError] = createSignal<string | null>(null);
  // R14': a snapshot repaint resets scroll and selection, so while the user is
  // reading history or selecting text the latest snapshot is parked here and
  // applied as soon as they return to the bottom / drop the selection.
  const [held, setHeld] = createSignal<string | null>(null);
  const userIsReading = () => {
    const buf = term.buffer.active;
    return term.hasSelection() || buf.viewportY < buf.baseY;
  };
  const applyHeld = () => {
    const text = held();
    if (text === null || disposed || userIsReading()) return;
    setHeld(null);
    render(text);
    lastText = text;
  };
  // One request in flight per view; a refresh asked for meanwhile marks the
  // view dirty and runs right after. (A "last wins" gate would starve rendering
  // whenever pane.read is slower than the poll period.) A pane switch mid-flight
  // is detected by `id !== p.paneId` and that response is dropped.
  let inFlight = false;
  let dirty = false;
  async function refresh() {
    if (disposed) return;
    if (inFlight) { dirty = true; return; }
    inFlight = true;
    const id = p.paneId;
    try {
      const { read } = await api.paneRead(id, Math.max(200, term.rows * 4));
      if (disposed || id !== p.paneId) return;
      setError(null);
      // Even revision = stable content (odd = a write was in flight); a known
      // even revision means the text cannot have changed. 0 = no revisions.
      if (read.revision !== 0 && read.revision % 2 === 0 && read.revision === lastRevision) return;
      lastRevision = read.revision;
      if (read.text === lastText) return;
      if (lastText !== "" && userIsReading()) { setHeld(read.text); return; }
      setHeld(null);
      render(read.text);
      lastText = read.text;
    } catch (err) {
      if (disposed || id !== p.paneId) return;
      console.error("pane.read failed", err);
      setError(String(err));
    } finally {
      inFlight = false;
      if (dirty && !disposed) { dirty = false; schedule(); }
    }
  }

  // The snapshot is formatted for the PTY's width (the TUI's, not ours): long
  // text wraps here (frames follow the wrapped rows); rule-only lines (input
  // box, separators) are cut to our width so they never wrap. The font never
  // changes. Columns are fitted to the host minus the viewport scrollbar plus
  // one spare cell, so neither text nor frames ever run under the scrollbar.
  const FONT = 13.5;
  function fitCols() {
    fit.fit();
    const vp = host.querySelector<HTMLElement>(".xterm-viewport");
    const scrollbarPx = vp ? vp.offsetWidth - vp.clientWidth : 0;
    const cellPx = host.clientWidth / Math.max(1, term.cols);
    const reserve = Math.ceil(scrollbarPx / cellPx) + 1;
    const cols = Math.max(20, term.cols - reserve);
    if (cols !== term.cols) term.resize(cols, term.rows);
  }

  function render(text: string) {
    if (disposed) return;
    const raw = text.replace(/\r\n?/g, "\n").split("\n").map((l) => fitRules(l, term.cols));
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
    term.write(out, () => {
      if (disposed) return;
      try {
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
        // Capped at the column count: every full-width frame ends on the same
        // line as the rules the CLI draws (fitRules cuts those to term.cols).
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

  const schedule = () => {
    if (disposed) return;
    clearTimeout(debounce);
    debounce = window.setTimeout(refresh, 120);
  };

  // Printable text → pane.send_text; control sequences → pane.send_keys names;
  // anything Herdr cannot name is dropped (never pasted as text, F5).
  function onData(data: string) {
    const input = mapInput(data);
    if ("drop" in input) { console.debug("input dropped:", input.reason); return; }
    const sent = "keys" in input ? api.sendKeys(p.paneId, input.keys) : api.sendText(p.paneId, input.text);
    sent.then(schedule, (e) => setError(String(e)));
  }

  onMount(() => {
    term = new Terminal({
      fontFamily: '"Cascadia Mono", Consolas, "JetBrains Mono", monospace',
      fontSize: FONT,
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
    fitCols();
    term.onData(onData);
    term.onScroll(() => applyHeld());
    term.onSelectionChange(() => applyHeld());
    let lastSize = "";
    const ro = new ResizeObserver(() => {
      const size = `${host.clientWidth}x${host.clientHeight}`;
      if (size === lastSize) return;
      lastSize = size;
      fitCols();
      if (lastText) render(lastText);
    });
    ro.observe(host);
    // Self-rescheduling poll so the period can relax once output events arrive.
    let poll: number | undefined;
    const schedulePoll = () => {
      poll = window.setTimeout(() => {
        refresh();
        if (!disposed) schedulePoll();
      }, seenTick ? POLL_MS_WITH_OUTPUT_EVENTS : POLL_MS_WITHOUT_OUTPUT_EVENTS);
    };
    schedulePoll();
    onCleanup(() => { ro.disconnect(); clearTimeout(poll); term.dispose(); });
    refresh();
  });

  // Output tick from App (routeEvent → "tick" for this pane): coalesce through
  // the same 120 ms debounce as own input; in-flight + dirty handles the rest.
  createEffect(on(() => p.outputTick, (tick) => {
    if (!tick) return;
    seenTick = true;
    schedule();
  }, { defer: true }));

  createEffect(on(() => p.paneId, () => {
    // New pane: nothing from the previous one may hold its first snapshot (R17 N1).
    lastText = "";
    lastRevision = 0;
    setHeld(null);
    term.clearSelection();
    term.scrollToBottom();
    refresh();
  }, { defer: true }));

  return (
    <div class="term">
      <div class="th"><span>◆ {header().title}</span><span class="micro">{header().where}</span><span class="micro right" style={error() ? "color:#FF2B4A" : held() !== null ? "color:#FCEE0A" : ""}>{error() ?? (held() !== null ? "NEW OUTPUT ↓ (scroll down)" : header().ctx)}</span></div>
      <div class="xterm-host" ref={host} />
    </div>
  );
}
