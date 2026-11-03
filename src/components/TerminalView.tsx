import { createEffect, createSignal, on, onCleanup, onMount } from "solid-js";
import { Terminal, type IDecoration } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { api, type PaneCursor, type PaneRead, type Tree } from "../api";
import { blockWidth, classify, fitRules, inputBoxStart, promptLine, stripAnsi, stripBackground, type LineKind } from "../dialogue";
import { mapInput } from "../keys";
import { planUpdate } from "../render-plan";
import { hotkeyOf } from "../actions";

const COLOR: Record<Exclude<LineKind, null>, string> = {
  user: "\x1b[38;2;255;246;168m",
  agent: "\x1b[38;2;143;245;184m",
};

/**
 * Pane view: an xterm that shows Herdr's ANSI snapshot of the pane
 * (`pane.read`) as a chat and forwards keystrokes with `pane.send_text` /
 * `pane.send_keys` (mapping in `src/keys.ts`). The snapshot is refreshed
 * shortly after own input, on the fork's `pane_output_changed` tick for this
 * pane (M1.5), and by a poll: 2.5 s until the first tick proves the server
 * emits output events, then a 10 s safety net (architecture.md F3).
 *
 * M2: snapshots are applied incrementally (`src/render-plan.ts`): the buffer
 * keeps everything written so far as history, and only the tail that changed
 * is rewritten in place, so scroll position, selection and scrollback survive
 * an update. A full repaint (`term.reset()`) happens only when the new
 * snapshot cannot be aligned with what is on screen (first render, pane
 * switch, resize). Dialogue blocks are recoloured and framed with xterm
 * decorations; the cursor comes from the fork's `pane.read` (`cursor.line`),
 * falling back to the end of the prompt line on upstream Herdr.
 */
export const POLL_MS_WITHOUT_OUTPUT_EVENTS = 2500;
export const POLL_MS_WITH_OUTPUT_EVENTS = 10000;

type Snapshot = { text: string; cursor: PaneCursor | null | undefined };

export default function TerminalView(p: { paneId: string; tree?: Tree; outputTick?: number }) {
  let host!: HTMLDivElement;
  let inputHost!: HTMLDivElement;
  // Two terminals: the chat scrolls; the CLI's input box (rules, prompt,
  // status line) is pinned below it and always visible.
  let term: Terminal;
  let inputTerm: Terminal;
  let fit: FitAddon;
  let last: Snapshot | null = null;
  // `revision` from the last applied pane.read (fork: content counter; upstream: always 0).
  let lastRevision = 0;
  // Set by the first output tick: the server emits output events, so the poll
  // may relax from 2.5 s to a 10 s safety net.
  let seenTick = false;
  let debounce: number | undefined;
  // Logical chat lines currently in the xterm buffer, as written (coloured),
  // with their dialogue kind: the `prev` side of `planUpdate`.
  let rendered: string[] = [];
  let renderedKinds: LineKind[] = [];
  let decorations: IDecoration[] = [];
  // Set on dispose: pending timers/promises must not touch the disposed xterm or signals.
  let disposed = false;
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
  // A full repaint resets scroll and selection, so when a snapshot needs one
  // while the user is reading history or selecting text, it is parked here and
  // applied as soon as they return to the bottom / drop the selection.
  // Incremental updates never need this: they leave the viewport alone.
  const [held, setHeld] = createSignal<Snapshot | null>(null);
  const userIsReading = () => {
    const buf = term.buffer.active;
    return term.hasSelection() || buf.viewportY < buf.baseY;
  };
  const applyHeld = () => {
    const snap = held();
    if (snap === null || disposed || userIsReading()) return;
    setHeld(null);
    render(snap, false);
    last = snap;
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
      const snap = toSnapshot(read);
      if (last && read.text === last.text) {
        // Same text, maybe a moved cursor: only the input box needs it.
        if (cursorKey(snap.cursor) !== cursorKey(last.cursor)) { renderInputFromText(snap); last = snap; }
        return;
      }
      // Hold only when the update would be a repaint while the user is reading.
      if (!render(snap, last !== null)) { setHeld(snap); return; }
      setHeld(null);
      last = snap;
    } catch (err) {
      if (disposed || id !== p.paneId) return;
      console.error("pane.read failed", err);
      setError(String(err));
    } finally {
      inFlight = false;
      if (dirty && !disposed) { dirty = false; schedule(); }
    }
  }
  const toSnapshot = (read: PaneRead): Snapshot => ({ text: read.text, cursor: read.cursor });
  const cursorKey = (c: PaneCursor | null | undefined) => (c ? `${c.line ?? "-"}:${c.col}:${c.visible}` : "");

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
    if (inputTerm.cols !== cols) inputTerm.resize(cols, inputTerm.rows);
  }

  /** Split a snapshot into the chat part (processed lines) and the input box. */
  function prepare(text: string) {
    const all = text.replace(/\r\n?/g, "\n").split("\n").map((l) => fitRules(l, term.cols));
    const allPlain = all.map(stripAnsi);
    const boxAt = inputBoxStart(allPlain);
    const raw = boxAt >= 0 ? all.slice(0, boxAt) : all;
    const rawPlain = boxAt >= 0 ? allPlain.slice(0, boxAt) : allPlain;
    const box = { raw: boxAt >= 0 ? all.slice(boxAt) : [], plain: boxAt >= 0 ? allPlain.slice(boxAt) : [], at: boxAt };
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
    const coloured = lines.map((line, i) => (kinds[i] ? `${COLOR[kinds[i]!]}${kinds[i] === "user" ? stripBackground(line) : line}\x1b[39m` : line));
    return { coloured, plain, kinds, box };
  }

  function renderInputFromText(snap: Snapshot) {
    const { box } = prepare(snap.text);
    renderInput(box.raw, box.plain, snap.cursor, box.at);
  }

  // xterm's write() is asynchronous and term.reset() does not flush data that
  // is already queued, so renders are serialised: while a write is pending the
  // newest request waits here and runs from the write callback.
  let writing = false;
  let pending: { snap: Snapshot; canHold: boolean } | null = null;
  function writeDone(after: () => void) {
    return () => {
      writing = false;
      if (!disposed) after();
      const next = pending;
      pending = null;
      if (next && !disposed) {
        if (render(next.snap, next.canHold)) { setHeld(null); last = next.snap; } else setHeld(next.snap);
      }
    };
  }

  /**
   * Apply a snapshot. Returns false (and changes nothing) when the update
   * would be a full repaint and `canHold` says the user is busy reading.
   * While a previous write is still pending the snapshot is queued and the
   * call reports success; the queued render decides about holding later.
   */
  function render(snap: Snapshot, canHold: boolean): boolean {
    if (disposed) return true;
    if (writing) { pending = { snap, canHold }; return true; }
    const { coloured, plain, kinds, box } = prepare(snap.text);
    let plan = planUpdate(rendered, coloured);
    // The buffer must still hold exactly what `rendered` says (xterm may have
    // trimmed scrollback); otherwise the alignment is meaningless.
    if (plan.kind === "rewrite" || plan.kind === "append") {
      if (rowsByLine().length !== rendered.length) plan = { kind: "reset", lines: coloured };
    }
    if (plan.kind === "rewrite") {
      const buf = term.buffer.active;
      const cutRow = rowsByLine()[plan.fromLine]?.[0];
      // Only viewport rows can be rewritten in place; a change above them is a repaint.
      if (cutRow === undefined || cutRow < buf.baseY) plan = { kind: "reset", lines: coloured };
    }
    if (plan.kind === "reset" && canHold && userIsReading()) return false;

    renderInput(box.raw, box.plain, snap.cursor, box.at);
    switch (plan.kind) {
      case "none":
        return true;
      case "append": {
        const first = rendered.length;
        rendered = rendered.concat(plan.lines);
        renderedKinds = renderedKinds.concat(kinds.slice(first));
        writing = true;
        term.write((first ? "\r\n" : "") + plan.lines.join("\r\n"), writeDone(() => decorateFrom(first, plain)));
        return true;
      }
      case "rewrite": {
        const buf = term.buffer.active;
        const cutRow = rowsByLine()[plan.fromLine][0];
        const up = buf.baseY + buf.cursorY - cutRow;
        for (const d of decorations.filter((d) => d.marker.line >= cutRow)) d.dispose();
        decorations = decorations.filter((d) => !d.isDisposed);
        rendered = rendered.slice(0, plan.fromLine).concat(plan.lines);
        renderedKinds = renderedKinds.slice(0, plan.fromLine).concat(kinds.slice(plan.fromLine));
        const from = plan.fromLine;
        writing = true;
        term.write(`${up > 0 ? `\x1b[${up}A` : ""}\r\x1b[J${plan.lines.join("\r\n")}`, writeDone(() => decorateFrom(from, plain)));
        return true;
      }
      case "reset": {
        for (const d of decorations) d.dispose();
        decorations = [];
        rendered = plan.lines;
        renderedKinds = kinds.slice();
        term.reset();
        writing = true;
        term.write(plan.lines.join("\r\n"), writeDone(() => decorateFrom(0, plain)));
        return true;
      }
    }
  }

  // Logical line → buffer rows (long lines wrap in xterm), up to the cursor's
  // row: rows below it are the viewport's empty padding, not written lines.
  function rowsByLine(): number[][] {
    const buf = term.buffer.active;
    const lastRow = Math.min(buf.length - 1, buf.baseY + buf.cursorY);
    const map: number[][] = [];
    for (let r = 0; r <= lastRow; r++) {
      const line = buf.getLine(r);
      if (!line) break;
      if (line.isWrapped && map.length) map[map.length - 1].push(r);
      else map.push([r]);
    }
    return map;
  }

  /** Frame the dialogue blocks from the block that contains `startLine` on. */
  function decorateFrom(startLine: number, plain: string[]) {
    if (disposed) return;
    try {
      const kinds = renderedKinds;
      let start = startLine;
      while (start > 0 && kinds[start - 1] !== null && kinds[start - 1] === kinds[Math.min(start, kinds.length - 1)]) start--;
      const rows = rowsByLine();
      const startRow = rows[start]?.[0];
      if (startRow !== undefined) {
        for (const d of decorations.filter((d) => d.marker.line >= startRow)) d.dispose();
        decorations = decorations.filter((d) => !d.isDisposed);
      }
      decorate(kinds, plain, rows, start);
    } catch (err) {
      console.error("decorate failed", err);
      setError(String(err));
    }
  }

  function decorate(kinds: LineKind[], plain: string[], rows: number[][], from: number) {
    const buf = term.buffer.active;
    const cursorRow = buf.baseY + buf.cursorY;
    let i = from;
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

  /**
   * The input box: fixed number of rows, no scrollback. The cursor goes where
   * the fork says (`cursor.line` relative to the box start); without that,
   * to the end of the prompt line as an estimate.
   */
  let inputWriting = false;
  let inputPending: [string[], string[], PaneCursor | null | undefined, number] | null = null;
  function renderInput(raw: string[], plain: string[], cursor: PaneCursor | null | undefined, boxAt: number) {
    if (inputWriting) { inputPending = [raw, plain, cursor, boxAt]; return; }
    while (raw.length && plain[plain.length - 1].trim() === "") { raw.pop(); plain.pop(); }
    const rows = raw.length;
    inputHost.style.display = rows ? "" : "none";
    if (!rows) { inputTerm.reset(); return; }
    if (inputTerm.rows !== rows) inputTerm.resize(inputTerm.cols, rows);
    inputTerm.reset();
    inputWriting = true;
    inputTerm.write(raw.join("\r\n"), () => {
      inputWriting = false;
      const next = inputPending;
      inputPending = null;
      if (next && !disposed) { renderInput(...next); return; }
      if (disposed) return;
      let row: number, col: number;
      if (cursor && cursor.line != null && boxAt >= 0 && cursor.line >= boxAt && cursor.line < boxAt + rows) {
        row = cursor.line - boxAt;
        col = Math.min(inputTerm.cols - 1, cursor.col);
        if (!cursor.visible) { inputTerm.options.cursorStyle = "underline"; inputTerm.options.cursorInactiveStyle = "none"; return; }
      } else {
        const idx = promptLine(plain);
        if (idx < 0) { inputTerm.options.cursorStyle = "underline"; inputTerm.options.cursorInactiveStyle = "none"; return; }
        const typed = [...plain[idx].replace(/\s+$/, "")].length;
        row = idx;
        col = Math.min(inputTerm.cols - 1, Math.max(typed, 2));
      }
      inputTerm.options.cursorStyle = "block";
      inputTerm.options.cursorInactiveStyle = "outline";
      inputTerm.write(`\x1b[${row + 1};${col + 1}H`);
    });
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

  /** Forget what is on screen so the next snapshot repaints from scratch. */
  function invalidate() {
    rendered = [];
    renderedKinds = [];
    for (const d of decorations) d.dispose();
    decorations = [];
  }

  onMount(() => {
    term = new Terminal({
      fontFamily: '"Cascadia Mono", Consolas, "JetBrains Mono", monospace',
      fontSize: FONT,
      lineHeight: 1.25,
      cursorBlink: false,
      cursorStyle: "underline",
      cursorInactiveStyle: "none",
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
    inputTerm = new Terminal({ ...term.options, scrollback: 0, rows: 1, cols: term.cols, cursorBlink: true });
    inputTerm.open(inputHost);
    inputTerm.onData(onData);
    // Keys typed anywhere in the pane go to the CLI; the caret lives in the input box.
    term.textarea?.addEventListener("focus", () => inputTerm.focus());
    // Deck's own keys (F1/F2/Ctrl+K, src/actions.ts) never reach the PTY even
    // if a listener order ever lets them past App's capture handler.
    const notForPty = (e: KeyboardEvent) => !(e.type === "keydown" && hotkeyOf(e, false) !== null);
    term.attachCustomKeyEventHandler(notForPty);
    inputTerm.attachCustomKeyEventHandler(notForPty);
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
      // New width → different wrapping and rule lengths: repaint from scratch.
      if (last) { invalidate(); render(last, false); }
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
    onCleanup(() => { ro.disconnect(); clearTimeout(poll); term.dispose(); inputTerm.dispose(); });
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
    // New pane: nothing from the previous one may hold its first snapshot (R17 N1),
    // and its history must not be aligned with the new pane's text.
    last = null;
    lastRevision = 0;
    invalidate();
    setHeld(null);
    term.clearSelection();
    term.scrollToBottom();
    refresh();
  }, { defer: true }));

  return (
    <div class="term">
      <div class="th"><span>◆ {header().title}</span><span class="micro">{header().where}</span><span class="micro right" style={error() ? "color:#FF2B4A" : held() !== null ? "color:#FCEE0A" : ""}>{error() ?? (held() !== null ? "NEW OUTPUT ↓ (scroll down)" : header().ctx)}</span></div>
      <div class="pane-body">
        <div class="xterm-host" ref={host} />
        <div class="xterm-input" ref={inputHost} />
      </div>
    </div>
  );
}
