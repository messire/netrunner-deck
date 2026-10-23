import { createEffect, on, onCleanup, onMount } from "solid-js";
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { api, type Tree } from "../api";

/**
 * Milestone 1 pane view: an xterm that shows Herdr's ANSI snapshot of the pane
 * (`pane.read`) and forwards keystrokes with `pane.send_text` / `pane.send_keys`.
 * Refreshes on `pane.updated` events (via `tick`) with a short debounce, plus a
 * slow fallback poll. Milestone 2 swaps the snapshot for a streaming attach.
 */
export default function TerminalView(p: { paneId: string; tick: number; tree?: Tree }) {
  let host!: HTMLDivElement;
  let term: Terminal;
  let fit: FitAddon;
  let lastText = "";
  let debounce: number | undefined;

  const pane = () => p.tree?.panes.find((x) => x.pane_id === p.paneId);
  const header = () => {
    const x = pane();
    if (!x) return { title: p.paneId, where: "", ctx: "" };
    const who = x.agent ? `${(x.display_agent ?? x.agent).toUpperCase()} · ${x.label ?? x.title ?? ""}` : (x.title ?? "SHELL");
    const ws = p.tree?.workspaces.find((w) => w.workspace_id === x.workspace_id)?.label ?? x.workspace_id;
    return { title: who, where: `${ws} · ${x.cwd ?? ""}`, ctx: x.tokens?.ctx ? `CTX ${x.tokens.ctx}%` : "" };
  };

  async function refresh() {
    try {
      const { read } = await api.paneRead(p.paneId, Math.max(200, term.rows * 4));
      if (read.text === lastText) return;
      lastText = read.text;
      term.reset();
      term.write(read.text.replace(/\n/g, "\r\n"));
    } catch {
      /* server went away; the offline banner covers it */
    }
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
      // Snapshot mode has no cursor position from the server; hide xterm's own
      // cursor so it does not sit at the end of the text. Streaming attach restores it.
      cursorBlink: false,
      cursorStyle: "bar",
      cursorInactiveStyle: "none",
      cursorWidth: 1,
      scrollback: 5000,
      allowProposedApi: true,
      theme: {
        background: "#080406", foreground: "#cfe9ea", cursor: "#080406", cursorAccent: "#080406",
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
    const ro = new ResizeObserver(() => fit.fit());
    ro.observe(host);
    const poll = setInterval(refresh, 2500);
    onCleanup(() => { ro.disconnect(); clearInterval(poll); term.dispose(); });
    refresh();
  });

  createEffect(on(() => p.paneId, () => { lastText = ""; refresh(); }, { defer: true }));
  createEffect(on(() => p.tick, schedule, { defer: true }));

  return (
    <div class="term">
      <div class="th"><span>◆ {header().title}</span><span class="micro">{header().where}</span><span class="micro right">{header().ctx}</span></div>
      <div class="xterm-host" ref={host} />
    </div>
  );
}
