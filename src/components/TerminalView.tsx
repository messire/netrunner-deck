import { createEffect, createSignal, on, onCleanup, onMount } from "solid-js";
import { Terminal, type IDecoration } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { api, type PaneCursor, type PaneRead, type Tree } from "../api";
import { blockWidth, classify, fitRules, inputBoxStart, promptLine, stripAnsi, stripBackground, wrapAnsi, type LineKind } from "../dialogue";
import { mapInput, pasteText, specialKeyDown } from "../keys";
import { planUpdate } from "../render-plan";
import { hotkeyOf } from "../actions";
import { throttleLeading } from "../events";
import type { MenuState } from "./ContextMenu";

const COLOR: Record<Exclude<LineKind, null>, string> = {
  user: "\x1b[38;2;255;246;168m",
  agent: "\x1b[38;2;143;245;184m",
};

/**
 * Pane view: an xterm that shows Herdr's ANSI snapshot of the pane
 * (`pane.read`) as a chat and forwards keystrokes with `pane.send_text` /
 * `pane.send_keys` (mapping in `src/keys.ts`). The snapshot is refreshed
 * shortly after own input, on the fork's `pane_output_changed` tick for this
 * pane (M1.5) — at once, then at most every `OUTPUT_THROTTLE_MS` while output
 * keeps coming — and by a poll: 2.5 s until the first tick proves the server
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
/**
 * Output ticks → `pane.read`: leading edge, then one read per window. The
 * server already coalesces output events to one per 100 ms per pane, so a
 * trailing debounce here (the old 120 ms) was reset by every event and never
 * fired while an agent streamed; the view froze until the output paused.
 */
export const OUTPUT_THROTTLE_MS = 100;

type Snapshot = { text: string; cursor: PaneCursor | null | undefined };

export default function TerminalView(p: { paneId: string; tree?: Tree; outputTick?: number; onMenu?: (menu: MenuState) => void }) {
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
  // Plain text of `rendered`, for frame widths: a snapshot's own line
  // indices stop matching `rendered` once its window has slid past history.
  let renderedPlain: string[] = [];
  let decorations: IDecoration[] = [];
  // Last few render steps, for the diagnostic record `verify` writes.
  const trail: string[] = [];
  const note = (s: string) => { trail.push(`${new Date().toISOString().slice(11, 23)} ${s}`); if (trail.length > 16) trail.shift(); };
  let lastHeal = 0;
  // Set on dispose: pending timers/promises must not touch the disposed xterm or signals.
  let disposed = false;
  onCleanup(() => { disposed = true; clearTimeout(debounce); kick.cancel(); });

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
    return term.hasSelection() || nativeSelection() !== null || buf.viewportY < buf.baseY;
  };

  // Text in the chat is selected the browser's way (DOM renderer rows are
  // real text), so a selection follows the words, not whole grid rows as in a
  // terminal. xterm's own drag-selection is kept out of the chat pane by
  // stopping its mousedown; the input box keeps the terminal behaviour.
  function nativeSelection(): string | null {
    const sel = typeof window !== "undefined" ? window.getSelection() : null;
    if (!sel || sel.isCollapsed || sel.rangeCount === 0 || !host) return null;
    const node = sel.anchorNode;
    if (!node || !host.contains(node)) return null;
    const text = sel.toString();
    return text.trim() ? text : null;
  }
  /** Row divs become lines; trailing pad goes, the common bubble indent too (relative indentation stays). */
  function tidySelection(text: string): string {
    const lines = text.replace(/\r/g, "").split("\n").map((l) => l.replace(/\s+$/, ""));
    while (lines.length && lines[0] === "") lines.shift();
    while (lines.length && lines[lines.length - 1] === "") lines.pop();
    const indents = lines.filter((l) => l.trim()).map((l) => /^ */.exec(l)![0].length);
    const common = indents.length ? Math.min(...indents) : 0;
    return lines.map((l) => l.slice(Math.min(common, l.length))).join("\n");
  }
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
      // An idle pane is where a broken view would stay broken, so the poll
      // doubles as the consistency check.
      if (read.revision !== 0 && read.revision % 2 === 0 && read.revision === lastRevision) { verify("poll"); return; }
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
      if (dirty && !disposed) { dirty = false; kick(); }
    }
  }
  // Output path: refresh now, then at most once per window while ticks keep coming.
  const kick = throttleLeading(() => { refresh(); }, OUTPUT_THROTTLE_MS);
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
    // The input box (live prompt) is already split off: no line left is the prompt.
    const rawKinds = classify(rawPlain, raw, { livePrompt: boxAt < 0 });
    // Dialogue blocks become chat bubbles: text re-wrapped to at most 80 % of
    // the width, one cell of inner margin on each side, a blank row inside the
    // bubble at the top and bottom; the user's bubbles sit at the right edge
    // (lines left-padded to the bubble's column), the agent's at the left.
    // `offsets[i]` is the bubble's first column for framed lines (0 otherwise).
    const bubbleMax = Math.max(20, Math.floor(term.cols * 0.8));
    const lines: string[] = [], plain: string[] = [], kinds: LineKind[] = [], offsets: number[] = [];
    const push = (l: string, pl: string, k: LineKind, off = 0) => { lines.push(l); plain.push(pl); kinds.push(k); offsets.push(off); };
    let i = 0;
    while (i < raw.length) {
      const k = rawKinds[i];
      if (!k) { push(raw[i], rawPlain[i], null); i++; continue; }
      let j = i;
      while (j + 1 < raw.length && rawKinds[j + 1] === k) j++;
      const body: string[] = [];
      for (let r = i; r <= j; r++) {
        const src = k === "user" ? stripBackground(raw[r]) : raw[r];
        body.push(...wrapAnsi(src, bubbleMax - 2));
      }
      const bodyPlain = body.map(stripAnsi);
      const width = blockWidth(bodyPlain, bubbleMax); // widest line + 2 (margins)
      const off = k === "user" ? Math.max(0, term.cols - 1 - width) : 0;
      const pad = " ".repeat(off + 1);
      // The pad before a right-aligned bubble is drawn in palette colour 16 so
      // its DOM spans get their own class (`xterm-fg-16`): a browser selection
      // across the bubble does not paint the empty space to its left.
      const padAnsi = `\x1b[38;5;16m${pad}${COLOR[k]}`;
      push(padAnsi, pad, k, off);
      for (let r = 0; r < body.length; r++) push(`${padAnsi}${body[r]}`, `${pad}${bodyPlain[r]}`, k, off);
      push(padAnsi, pad, k, off);
      i = j + 1;
    }
    const coloured = lines.map((line, i) => (kinds[i] ? `${COLOR[kinds[i]!]}${line}\x1b[39m` : line));
    return { coloured, plain, kinds, offsets, box };
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
      } else if (!disposed) {
        // The DOM paints on the next frame; check once it has.
        window.setTimeout(() => verify("write"), 80);
      }
    };
  }

  /**
   * Consistency check of what the buffer, the frames and the DOM show against
   * what the view believes it rendered. The view has been seen with an empty
   * chat and every frame label on one row, from a sequence nobody could
   * reproduce (2026-11-05); until the cause is known, a failed check is
   * recorded in the diagnostic log with the state that led to it, and the
   * view repaints from scratch (parked behind the NEW OUTPUT banner when the
   * user is reading history, like any other repaint).
   */
  function verify(where: string): boolean {
    if (disposed || writing || !last || rendered.length === 0) return true;
    const buf = term.buffer.active;
    const rows = rowsByLine();
    const problems: string[] = [];
    if (rows.length !== rendered.length) problems.push(`buffer holds ${rows.length} lines, view rendered ${rendered.length}`);
    let lastText = -1;
    for (let i = renderedPlain.length - 1; i >= 0; i--) if (renderedPlain[i].trim() !== "") { lastText = i; break; }
    if (lastText >= 0 && rows[lastText]) {
      const got = rows[lastText].map((r) => buf.getLine(r)?.translateToString(true) ?? "").join("");
      if (letters(got) !== letters(renderedPlain[lastText])) problems.push(`line ${lastText} reads "${got.trim().slice(0, 50)}", expected "${renderedPlain[lastText].trim().slice(0, 50)}"`);
    }
    const lines = decorations.map((d) => d.marker.line);
    if (lines.some((l) => l < 0 || l >= buf.length)) problems.push("a frame marker lies outside the buffer");
    if (new Set(lines).size !== lines.length) problems.push("two frames on one buffer row");
    // DOM positions are stale while the window is hidden (no frames are painted).
    const visible = document.visibilityState === "visible";
    const labels = visible ? [...host.querySelectorAll<HTMLElement>(".dlg-top, .dlg-only")].filter((e) => e.style.display !== "none") : [];
    const tops = labels.map((e) => e.style.top);
    if (new Set(tops).size !== tops.length) problems.push(`${tops.length - new Set(tops).size} frame labels share a screen row`);
    if (problems.length === 0) return true;
    const now = Date.now();
    const dims = (term as unknown as { _core?: { _renderService?: { dimensions?: { css?: { cell?: unknown } } } } })._core?._renderService?.dimensions?.css?.cell;
    // Every label that shares its `top` with another (seen live: all of them
    // with an empty top, right after a one-row height change), with the
    // decoration it belongs to, so the next record names the mechanism.
    const hostTop = host.getBoundingClientRect().top;
    const shared = labels.filter((e) => tops.indexOf(e.style.top) !== tops.lastIndexOf(e.style.top)).slice(0, 8).map((e) => ({
      css: e.style.cssText.slice(0, 160), rect: Math.round(e.getBoundingClientRect().top - hostTop), who: e.dataset.who,
      line: decorations.find((d) => d.element === e)?.marker.line ?? null, inDom: e.isConnected,
    }));
    const record = {
      at: new Date().toISOString(), where, pane: p.paneId, problems, trail,
      term: { rows: term.rows, cols: term.cols, length: buf.length, baseY: buf.baseY, viewportY: buf.viewportY, cursorY: buf.cursorY, cell: dims },
      view: { rendered: rendered.length, rows: rows.length, decorations: lines.length, markers: lines.slice(0, 60), domTops: tops.slice(0, 40), shared, held: held() !== null, visible, hostPx: `${host.clientWidth}x${host.clientHeight}` },
      heal: now - lastHeal > 5000 ? "repaint" : "skipped (healed <5 s ago)",
    };
    console.warn("pane view inconsistent", record);
    api.diagLog(JSON.stringify(record)).catch(() => {});
    if (now - lastHeal > 5000) {
      lastHeal = now;
      note(`heal@${where}`);
      invalidate();
      if (userIsReading()) setHeld(last); else render(last, false);
    }
    return false;
  }
  /** Letters and digits only: cell widths, combining marks and spacing may differ between source and buffer. */
  const letters = (s: string) => s.replace(/[^\p{L}\p{N}]/gu, "");

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
    if (plan.kind === "reset" && canHold && userIsReading()) { note("reset held"); return false; }

    renderInput(box.raw, box.plain, snap.cursor, box.at);
    // `kinds`/`plain` index the snapshot; `rendered` may start earlier (history
    // that scrolled out of the snapshot window): offset = rendered.length - snapshot.length
    // for append/rewrite, where the snapshot's tail is the end of `rendered`.
    switch (plan.kind) {
      case "none":
        return true;
      case "append": {
        const first = rendered.length;
        const offset = first + plan.lines.length - coloured.length;
        rendered = rendered.concat(plan.lines);
        renderedKinds = renderedKinds.concat(kinds.slice(first - offset));
        renderedPlain = renderedPlain.concat(plain.slice(first - offset));
        note(`append ${plan.lines.length} at ${first}`);
        writing = true;
        term.write((first ? "\r\n" : "") + plan.lines.join("\r\n"), writeDone(() => decorateFrom(first)));
        return true;
      }
      case "rewrite": {
        const buf = term.buffer.active;
        const cutRow = rowsByLine()[plan.fromLine][0];
        const up = buf.baseY + buf.cursorY - cutRow;
        for (const d of decorations.filter((d) => d.marker.line >= cutRow)) d.dispose();
        decorations = decorations.filter((d) => !d.isDisposed);
        const from = plan.fromLine;
        const offset = from + plan.lines.length - coloured.length;
        // With nothing to write the cursor stays on the erased row, which the
        // buffer then holds as one blank line: account for it.
        const blank = plan.lines.length === 0;
        rendered = rendered.slice(0, from).concat(blank ? [""] : plan.lines);
        renderedKinds = renderedKinds.slice(0, from).concat(blank ? [null] : kinds.slice(from - offset));
        renderedPlain = renderedPlain.slice(0, from).concat(blank ? [""] : plain.slice(from - offset));
        note(`rewrite ${plan.lines.length} from ${from} (row ${cutRow}, up ${up})`);
        writing = true;
        term.write(`${up > 0 ? `\x1b[${up}A` : ""}\r\x1b[J${plan.lines.join("\r\n")}`, writeDone(() => decorateFrom(from)));
        return true;
      }
      case "reset": {
        for (const d of decorations) d.dispose();
        decorations = [];
        rendered = plan.lines;
        renderedKinds = kinds.slice();
        renderedPlain = plain.slice();
        note(`reset ${plan.lines.length}`);
        term.reset();
        writing = true;
        term.write(plan.lines.join("\r\n"), writeDone(() => decorateFrom(0)));
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

  /** Frame the dialogue blocks from the block that contains `startLine` on (indices into `rendered`). */
  function decorateFrom(startLine: number) {
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
      decorate(kinds, renderedPlain, rows, start);
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
        // The block's first row is its blank spacer: pure padding, so its
        // length is the bubble's column + 1 (inner margin). Width = widest
        // line from that column, plus the right margin, capped at the edge.
        const off = Math.max(0, plain[i].length - 1);
        const widest = blockWidth(plain.slice(i, j + 1), 1 << 20) - 2;
        const width = Math.max(4, Math.min(term.cols - off, widest - off + 1));
        for (let r = first; r <= last; r++) {
          const marker = term.registerMarker(r - cursorRow);
          if (!marker) continue;
          const deco = term.registerDecoration({ marker, x: off, width, layer: "bottom" });
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

  /** ctrl+v / shift+insert: clipboard text → `pane.send_text` (Herdr brackets it, F5). */
  async function paste() {
    const id = p.paneId;
    try {
      const text = pasteText(await api.clipboardReadText());
      if (!text || disposed || id !== p.paneId) return;
      await api.sendText(id, text);
      schedule();
    } catch (err) {
      if (!disposed) setError(String(err));
    }
  }

  const hasSelection = () => nativeSelection() !== null || term.hasSelection() || inputTerm.hasSelection();
  const clearSelections = () => { window.getSelection()?.removeAllRanges(); term.clearSelection(); inputTerm.clearSelection(); };

  /** ctrl+c with a selection / ctrl+shift+c / ctrl+insert / menu COPY: the selection → clipboard, then deselect. */
  async function copy() {
    const native = nativeSelection();
    const text = native !== null ? tidySelection(native) : term.hasSelection() ? term.getSelection() : inputTerm.getSelection();
    if (!text) return;
    try {
      await api.clipboardWriteText(text);
      clearSelections();
    } catch (err) {
      if (!disposed) setError(String(err));
    }
  }

  /** Right-click anywhere in the pane: the terminal's own menu, not WebView2's. */
  function openMenu(e: MouseEvent) {
    e.preventDefault();
    e.stopPropagation();
    const selected = hasSelection();
    p.onMenu?.({
      x: e.clientX, y: e.clientY, title: header().title,
      items: [
        { label: "COPY", hint: "ctrl+c", disabled: !selected, run: () => void copy() },
        { label: "PASTE", hint: "ctrl+v", run: () => void paste() },
        { label: "SELECT ALL", run: () => term.selectAll() },
        { label: "CLEAR SELECTION", disabled: !selected, run: clearSelections },
      ],
    });
  }

  /** Forget what is on screen so the next snapshot repaints from scratch. */
  function invalidate() {
    rendered = [];
    renderedKinds = [];
    renderedPlain = [];
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
    // if a listener order ever lets them past App's capture handler. Keys xterm
    // would mangle (shift+enter → `\r`, ctrl+v → 0x16) are handled here on
    // keydown; preventDefault also stops the browser's own paste event, which
    // xterm would otherwise forward as a second copy of the text. ctrl+c with
    // a selection copies it (the selection lives in the chat xterm, the focus
    // in the input box, so the browser's own copy would copy nothing).
    const notForPty = (e: KeyboardEvent) => {
      if (e.type === "keydown" && hotkeyOf(e, false) !== null) return false;
      const special = specialKeyDown(e, hasSelection());
      if (!special) return true;
      e.preventDefault();
      if ("paste" in special) void paste();
      else if ("copy" in special) void copy();
      else api.sendKeys(p.paneId, special.keys).then(schedule, (err) => setError(String(err)));
      return false;
    };
    term.attachCustomKeyEventHandler(notForPty);
    inputTerm.attachCustomKeyEventHandler(notForPty);
    fitCols();
    term.onData(onData);
    term.onScroll(() => applyHeld());
    term.onSelectionChange(() => applyHeld());
    // Left-button presses in the chat never reach xterm's selection service;
    // the browser selects text instead. A plain click (nothing selected) puts
    // the caret back into the input box, as xterm's own mousedown would have.
    host.addEventListener("mousedown", (e) => { if (e.button === 0) e.stopPropagation(); }, true);
    host.addEventListener("mouseup", () => { if (nativeSelection() === null) inputTerm.focus(); });
    const onSel = () => applyHeld();
    document.addEventListener("selectionchange", onSel);
    onCleanup(() => document.removeEventListener("selectionchange", onSel));
    let lastSize = "";
    const ro = new ResizeObserver(() => {
      const size = `${host.clientWidth}x${host.clientHeight}`;
      if (size === lastSize) return;
      lastSize = size;
      fitCols();
      note(`resize ${size} → ${term.cols}x${term.rows}`);
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

  // Output tick from App (routeEvent → "tick" for this pane): read at once,
  // throttled while output streams; in-flight + dirty handles overlap.
  createEffect(on(() => p.outputTick, (tick) => {
    if (!tick) return;
    seenTick = true;
    kick();
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
    <div class="term" onContextMenu={openMenu}>
      <div class="th"><span>◆ {header().title}</span><span class="micro">{header().where}</span><span class="micro right" style={error() ? "color:#FF2B4A" : held() !== null ? "color:#FCEE0A" : ""}>{error() ?? (held() !== null ? "NEW OUTPUT ↓ (scroll down)" : header().ctx)}</span></div>
      <div class="pane-body">
        <div class="xterm-host" ref={host} />
        <div class="xterm-input" ref={inputHost} />
      </div>
    </div>
  );
}
