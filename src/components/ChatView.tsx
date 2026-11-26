import { For, Show, createEffect, createMemo, createSignal, on, onCleanup, onMount, type JSX } from "solid-js";
import { marked } from "marked";
import DOMPurify from "dompurify";
import { api, type HarnessRepo, type Tree } from "../api";
import { slashCurrent, slashItems, slashMatches, slashQuery } from "../slash";
import Think from "./Think";
import { codeBody, codeSpans } from "../codespans";
import { ChatBuilder, backgroundTasks, fmtDuration, parseLine, turnFooters, type Block, type ChatItem, type ChatState, type ToolCall } from "../transcript";
import { MODE_LABEL, cliMode, inputHint, menuOf, type CliMenu, type InputHint } from "../dialogue";
import { IMAGE_PASTE_KEY } from "../keys";
import { debounceTrailing } from "../events";
import { serverOf } from "../servers";
import AskCard from "./AskCard";
import { MODE_HINT, MODE_TITLE, PERMISSION_MODES, agentStatus, type AgentLive } from "../agent";

/**
 * Deck's own chat for a pane that runs Claude Code: the conversation is read
 * from the agent's transcript (`~/.claude/projects/<cwd>/<session>.jsonl`),
 * which Claude Code appends to as it works, so nothing is scraped from the
 * terminal. Input goes to the pane through Herdr (`pane.send_text` + Enter).
 * The terminal view stays one click away for everything the transcript does
 * not carry (permission prompts, slash-command menus, the CLI's own UI).
 *
 * A headless pane (`agent`: Claude Code run by `deck-server agent`, see
 * docs/adr/0003-headless-claude.md) has no terminal at all: input, images,
 * interrupt, model and mode go through `deck.agent.*`, and its event stream
 * adds the reply being written and the questions waiting for an answer.
 */
const TAIL_BYTES = 2 * 1024 * 1024;

// Claude writes with single newlines and space-indented bullets (it renders for a
// terminal), so a newline is a line break and indentation is kept (pre-wrap).
marked.setOptions({ gfm: true, breaks: true });
const md = (text: string): string => DOMPurify.sanitize(marked.parse(text, { async: false }) as string, { USE_PROFILES: { html: true } });

/**
 * Unsent input per pane. One ChatView is reused across tabs (only its props
 * change) and is unmounted when a terminal pane is shown, so each pane's draft
 * and pasted-image thumbnails live here and come back when its tab is selected.
 */
const drafts = new Map<string, { text: string; pasted: string[] }>();

/**
 * Chats already read, by session id. Showing the same session again (the
 * chat remounted, a tab switched back, or the id briefly unknown while the
 * live link reconnects) continues from here instead of emptying the chat and
 * reading the transcript again, which on a remote server takes a moment and
 * made the history blink. A few recent ones are kept; their images' blob:
 * URLs go with them.
 */
interface Folded { builder: ChatBuilder; offset: number; truncated: boolean; blobs: string[] }
const folded = new Map<string, Folded>();
const KEEP_CHATS = 4;
function foldedChat(id: string): Folded {
  const f = folded.get(id) ?? { builder: new ChatBuilder(), offset: 0, truncated: false, blobs: [] };
  folded.delete(id);
  folded.set(id, f); // most recent last
  for (const [old, o] of folded) {
    if (folded.size <= KEEP_CHATS) break;
    folded.delete(old);
    for (const u of o.blobs) URL.revokeObjectURL(u);
  }
  return f;
}

/** The image shown full-size over the chat (one per window). */
const [zoom, setZoom] = createSignal<string | null>(null);

/** `23:54` today, `17.11 23:54` on another day. */
const clock = (ts: number) => {
  if (!ts) return "";
  const d = new Date(ts);
  const t = d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
  return d.toDateString() === new Date().toDateString() ? t : `${String(d.getDate()).padStart(2, "0")}.${String(d.getMonth() + 1).padStart(2, "0")} ${t}`;
};

export default function ChatView(p: { paneId: string; sessionId: string; tree?: Tree; outputTick?: number; harness?: HarnessRepo[]; onTerminal?: () => void; agent?: () => AgentLive | undefined;
  /** Name of the server (machine) the session runs on. */
  machine?: string;
  /** Headless: the sign-in card, when the session's machine has no usable Claude login. */
  signIn?: () => JSX.Element | null }) {
  const headless = () => !!p.agent;
  const live = () => p.agent?.();
  // The chat is folded incrementally: only new transcript blocks are pushed,
  // and only touched items come back as new objects (the keyed <For> below
  // re-renders just those). Nothing keeps the raw blocks around.
  let cur: Folded = { builder: new ChatBuilder(), offset: 0, truncated: false, blobs: [] };
  const [chat, setChat] = createSignal<ChatState>({ items: [], title: null, pendingTools: 0 });
  // Pasted images arrive as base64 inside the transcript; they are turned into
  // blob: URLs once and the base64 is dropped, so a long session with many
  // screenshots does not keep megabytes of text alive twice (block + DOM).
  const toBlob = (src: string): string => {
    const m = /^data:([^;,]+);base64,(.*)$/s.exec(src);
    if (!m) return src;
    try {
      const bin = atob(m[2]);
      const bytes = new Uint8Array(bin.length);
      for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);
      const url = URL.createObjectURL(new Blob([bytes], { type: m[1] }));
      cur.blobs.push(url);
      return url;
    } catch { return src; }
  };
  const ingest = (fresh: Block[]) => {
    for (const b of fresh) if (b.kind === "user" && b.images.length) b.images = b.images.map(toBlob);
    cur.builder.push(fresh);
    setChat(cur.builder.state());
  };
  const [error, setError] = createSignal<string | null>(null);
  const [truncated, setTruncated] = createSignal(false);
  // The transcript has been read once for the session shown (until then the
  // live reply is not shown on its own, without the history above it).
  const [loaded, setLoaded] = createSignal(false);
  const [draft, setDraft] = createSignal(drafts.get(p.paneId)?.text ?? "");
  const [atBottom, setAtBottom] = createSignal(true);
  let loading: Promise<void> | null = null;
  let scroller!: HTMLDivElement;
  let input!: HTMLTextAreaElement;
  let disposed = false;

  // `✻ 13s · done 23:54` after each answered turn; the open turn gets its footer once the agent stops.
  const footers = createMemo(() => turnFooters(chat().items));
  const pane = () => p.tree?.panes.find((x) => x.pane_id === p.paneId);
  const status = () => {
    const a = live();
    if (a) return agentStatus(a);
    return p.tree?.agents.find((a) => a.pane_id === p.paneId)?.agent_status ?? pane()?.agent_status ?? "unknown";
  };
  const header = () => {
    const x = pane();
    if (!x) return { title: p.paneId, where: "" };
    const who = `${headless() ? "CLAUDE" : (x.display_agent ?? x.agent ?? "CLAUDE").toUpperCase()} · ${chat().title ?? x.label ?? x.title ?? ""}`;
    const ws = p.tree?.workspaces.find((w) => w.workspace_id === x.workspace_id)?.label ?? x.workspace_id;
    return { title: who, where: `${ws} · ${x.cwd ?? ""}` };
  };

  /** Read what the transcript has past `offset` (or its tail on first load). */
  const pull = (): Promise<void> => {
    if (loading) return loading;
    // A headless session that has not said anything yet has no id or transcript.
    if (!p.sessionId) return Promise.resolve();
    const f = cur;
    loading = (async () => {
      try {
        const first = f.offset === 0;
        const chunk = await api.transcriptRead(p.sessionId, first ? undefined : f.offset, first ? TAIL_BYTES : undefined, serverOf(p.paneId));
        if (disposed || chunk.session_id !== p.sessionId || f !== cur) return;
        if (first && chunk.start > 0) { f.truncated = true; setTruncated(true); }
        setLoaded(true);
        if (chunk.end > f.offset) {
          f.offset = chunk.end;
          const fresh = chunk.lines.flatMap(parseLine);
          if (fresh.length) ingest(fresh);
        }
        setError(null);
      } catch (e) {
        if (f === cur) { setError(String(e)); setLoaded(true); }
      } finally {
        loading = null;
        // The session changed while this read was out: read the new one.
        if (f !== cur && !disposed) void pull();
      }
    })();
    return loading;
  };

  onMount(() => {
    let unlisten: (() => void) | null = null;
    api.onTranscript((e) => { if (e.session_id === p.sessionId && e.len > cur.offset) void pull(); }).then((u) => { if (disposed) u(); else unlisten = u; });
    onCleanup(() => { disposed = true; unlisten?.(); api.transcriptWatch(null).catch(() => {}); });
  });
  let shownPane = "";
  createEffect(on(() => [p.sessionId, p.paneId] as const, ([id, pane]) => {
    // A headless pane's id drops to "" for a moment while its live link
    // reconnects; that is still the same conversation, so it stays on screen.
    if (!id && pane === shownPane && cur.offset > 0) return;
    shownPane = pane;
    cur = id ? foldedChat(id) : { builder: new ChatBuilder(), offset: 0, truncated: false, blobs: [] };
    setChat(cur.builder.state()); setTruncated(cur.truncated); setLoaded(cur.offset > 0); setError(null);
    if (id) api.transcriptWatch(id, serverOf(p.paneId)).catch((e) => setError(String(e)));
    void pull();
  }));

  // Follow new messages unless the user scrolled up to read.
  const onScroll = () => setAtBottom(scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight < 24);
  createEffect(on(() => chat().items.length, () => { if (atBottom()) queueMicrotask(() => scroller.scrollTo({ top: scroller.scrollHeight })); }));

  const send = async () => {
    const text = draft().trim();
    if (!text && !pasted().length) return;
    if (headless()) {
      const images = pasted().map((src) => /^data:([^;,]+);base64,(.*)$/s.exec(src)).filter((m): m is RegExpExecArray => !!m).map((m) => ({ media_type: m[1], data: m[2] }));
      try {
        await api.agentCall(p.paneId, "send", { text, images });
        lastSent = text;
        setDraft(""); setPasted([]); histAt = -1; histDraft = "";
        input.style.height = "";
      } catch (e) { setError(String(e)); }
      return;
    }
    try {
      if (text) await api.sendText(p.paneId, text);
      await api.sendKeys(p.paneId, ["enter"]);
      lastSent = text;
      setHint((h) => ({ ...h, suggestion: "" }));
      setDraft(""); setPasted([]); histAt = -1; histDraft = "";
      setTimeout(readHint, 600); setTimeout(readHint, 2000);
      input.style.height = "";
    } catch (e) { setError(String(e)); }
  };
  // The agent's input holds the images as `[Image #N]` placeholders; one
  // backspace there removes the last one (the CLI's cursor sits at its end),
  // so only the last pasted image can be dropped — like in the terminal.
  const dropLastImage = () => {
    if (!pasted().length) return;
    if (headless()) { setPasted((l) => l.slice(0, -1)); return; }
    api.sendKeys(p.paneId, ["backspace"]).then(() => setPasted((l) => l.slice(0, -1)), (err) => setError(String(err)));
  };
  // ↑/↓ walk the session's own prompts (newest first) when the caret is on the
  // first/last line of the draft, like the CLI's history; the unsent draft is
  // kept at position -1 and comes back on the way down.
  const history = createMemo(() => {
    const out: string[] = [];
    for (const it of chat().items) if (it.kind === "user" && it.text && out[out.length - 1] !== it.text) out.push(it.text);
    return out.reverse();
  });
  let histAt = -1;
  let histDraft = "";
  createEffect(on(() => p.sessionId, () => { histAt = -1; histDraft = ""; }, { defer: true }));
  const onFirstLine = () => !input.value.slice(0, input.selectionStart).includes("\n");
  const onLastLine = () => !input.value.slice(input.selectionEnd).includes("\n");
  const browse = (dir: 1 | -1): boolean => {
    const h = history();
    const next = histAt + dir;
    if (next < -1 || next >= h.length) return false;
    if (histAt === -1) histDraft = draft();
    histAt = next;
    setDraft(next === -1 ? histDraft : h[next]);
    queueMicrotask(() => { grow(); input.setSelectionRange(input.value.length, input.value.length); });
    return true;
  };
  // `/…` at the start of the draft: a popup over the harness (skills and
  // commands of this pane's repository and the personal ones) and the CLI's
  // built-ins; ↑/↓ pick, Tab/Enter complete to `/name `, Esc closes.
  const slashAll = createMemo(() => {
    const own = live()?.commands ?? [];
    if (!headless() || !own.length) return slashItems(p.harness ?? [], pane()?.cwd);
    // Headless: the session lists what it takes (its skills, commands and the built-ins that work without a terminal).
    const known = new Map(slashItems(p.harness ?? [], pane()?.cwd).filter((x) => x.kind !== "builtin").map((x) => [x.name, x]));
    return own.map((c) => known.get(c.name) ?? { name: c.name, kind: "builtin" as const, description: c.description, source: "claude code", args: c.argumentHint || undefined });
  });
  const slashQ = () => slashQuery(draft());
  const slash = createMemo(() => { const q = slashQ(); return q === null ? [] : slashMatches(slashAll(), q); });
  const [slashAt, setSlashAt] = createSignal(0);
  const [slashOff, setSlashOff] = createSignal(false); // Esc hides the popup for the current word
  createEffect(on(slashQ, () => { setSlashAt(0); setSlashOff(false); }));
  const slashOpen = () => slash().length > 0 && !slashOff();
  // The command already typed: its argument hint and description stay visible above the input while arguments are written.
  const typed = createMemo(() => slashCurrent(slashAll(), draft()));
  const complete = (name: string) => { setDraft(`/${name} `); queueMicrotask(() => { grow(); input.setSelectionRange(input.value.length, input.value.length); input.focus(); }); };

  const onKey = (e: KeyboardEvent) => {
    if (slashOpen()) {
      const n = slash().length;
      if (e.key === "ArrowDown") { e.preventDefault(); setSlashAt((i) => (i + 1) % n); return; }
      if (e.key === "ArrowUp") { e.preventDefault(); setSlashAt((i) => (i - 1 + n) % n); return; }
      if (e.key === "Tab" || (e.key === "Enter" && !e.shiftKey)) { e.preventDefault(); complete(slash()[slashAt()].name); return; }
      if (e.key === "Escape") { e.preventDefault(); setSlashOff(true); return; }
    }
    if (e.key === "ArrowUp" && !e.shiftKey && !e.ctrlKey && !e.altKey && onFirstLine() && browse(1)) { e.preventDefault(); return; }
    if (e.key === "ArrowDown" && !e.shiftKey && !e.ctrlKey && !e.altKey && onLastLine() && histAt >= 0 && browse(-1)) { e.preventDefault(); return; }
    if (e.key === "Tab" && !e.shiftKey && takeGhost()) { e.preventDefault(); return; }
    if (e.key === "Enter" && !e.shiftKey) { e.preventDefault(); void send(); }
    else if (e.key === "Backspace" && !draft() && pasted().length) { e.preventDefault(); dropLastImage(); }
    else if (e.key === "Escape") { void (headless() ? api.agentCall(p.paneId, "interrupt") : api.sendKeys(p.paneId, ["escape"])).catch(() => {}); }
  };
  // An image on the clipboard cannot travel as text: forward the CLI's own
  // image-paste key (Alt+V on Windows, Ctrl+V elsewhere) to the pane and
  // Claude Code reads the clipboard itself (shows `[Image #N]`).
  // Thumbnails of the images pasted for the next message (the agent holds the bytes).
  const [pasted, setPasted] = createSignal<string[]>(drafts.get(p.paneId)?.pasted ?? []);
  // The pane this draft belongs to: props may already name the next pane by the time cleanup runs.
  let draftPane = p.paneId;
  const keep = (id: string) => {
    const text = draft(), imgs = pasted();
    if (text || imgs.length) drafts.set(id, { text, pasted: imgs }); else drafts.delete(id);
  };
  createEffect(on(() => p.paneId, (id, prev) => {
    if (prev === undefined || prev === id) return;
    keep(prev); draftPane = id;
    const d = drafts.get(id);
    setDraft(d?.text ?? ""); setPasted(d?.pasted ?? []); histAt = -1; histDraft = "";
    queueMicrotask(grow);
  }));
  onCleanup(() => keep(draftPane));
  const onPaste = (e: ClipboardEvent) => {
    const files = [...(e.clipboardData?.items ?? [])].filter((i) => i.kind === "file" && i.type.startsWith("image/")).map((i) => i.getAsFile()).filter((f): f is File => !!f);
    if (!files.length) return;
    if (e.clipboardData?.getData("text/plain")) return; // text wins; the image is a preview of it
    e.preventDefault();
    const file = files[0];
    const keep = (): Promise<unknown> => (headless() ? Promise.resolve() : api.sendKeys(p.paneId, [IMAGE_PASTE_KEY]));
    keep().then(() => {
      const reader = new FileReader();
      reader.onload = () => setPasted((l) => l.concat(String(reader.result)));
      reader.readAsDataURL(file);
    }, (err) => setError(String(err)));
  };
  // A new session in the same pane starts with an empty agent input; a tab switch keeps the pane's own (see `keep`).
  createEffect(on(() => [p.paneId, p.sessionId] as const, ([pane], prev) => { if (prev?.[0] === pane) setPasted([]); }, { defer: true }));
  const grow = () => { input.style.height = ""; input.style.height = `${Math.min(input.scrollHeight, 180)}px`; syncHl(); };
  // Code spans in the draft get a chip painted behind the textarea's own text
  // (a textarea cannot style parts of its text); the layer scrolls with it.
  let hl!: HTMLDivElement;
  const syncHl = () => { if (hl && input) hl.scrollTop = input.scrollTop; };

  // Activity line under the log while the agent works (Herdr's status, the
  // same signal the sidebar uses): a pulsing marker, a verb and the elapsed
  // time since the status went to working — what the CLI shows as `· Ripping…`.
  const VERBS = ["Jacking in", "Ripping", "Decrypting", "Compiling", "Tracing", "Routing", "Splicing", "Hacking"];
  const [since, setSince] = createSignal<number | null>(null);
  const [now, setNow] = createSignal(Date.now());
  createEffect(on(status, (st) => setSince(st === "working" ? (since() ?? Date.now()) : null)));
  onMount(() => { const t = setInterval(() => setNow(Date.now()), 1000); onCleanup(() => clearInterval(t)); });
  const elapsed = () => { const s = since(); if (s === null) return ""; const d = Math.max(0, Math.round((now() - s) / 1000)); return d >= 60 ? `${Math.floor(d / 60)}m ${d % 60}s` : `${d}s`; };
  const verb = () => { const s = since(); return s === null ? "" : VERBS[Math.floor((now() - s) / 8000) % VERBS.length]; };

  // The CLI's own input box, read from the pane after output settles: its
  // ghost suggestion is shown in Deck's input (Tab takes it, as in the CLI)
  // and its status line ("bypass permissions on …") under the input.
  const [hint, setHint] = createSignal<InputHint>({ suggestion: "", status: "", below: [], activity: "" });
  // A numbered menu the CLI is showing (permission prompt, AskUserQuestion…),
  // drawn as a card; a click moves the CLI's cursor with arrow keys and Enter.
  const [menu, setMenu] = createSignal<CliMenu | null>(null);
  const [choosing, setChoosing] = createSignal(false);
  const readHint = debounceTrailing(() => {
    if (headless()) return; // no terminal to read
    const id = p.paneId;
    api.paneRead(id, 40).then(({ read }) => {
      if (disposed || id !== p.paneId) return;
      setHint(inputHint(read.text));
      // a menu is only a menu while the agent waits for an answer
      if (!choosing()) setMenu(status() === "blocked" ? menuOf(read.text) : null);
    }, () => {});
  }, 300);
  const choose = async (index: number) => {
    const m = menu();
    if (!m || choosing()) return;
    setChoosing(true);
    try {
      const d = index - m.current;
      const keys = Array.from({ length: Math.abs(d) }, () => (d > 0 ? "down" : "up"));
      if (keys.length) await api.sendKeys(p.paneId, keys);
      await api.sendKeys(p.paneId, ["enter"]);
      setMenu(null);
    } catch (e) { setError(String(e)); } finally { setChoosing(false); readHint(); }
  };
  createEffect(on(() => [p.outputTick, p.paneId, status()], () => { if (status() !== "blocked") setMenu(null); readHint(); }));
  onCleanup(() => readHint.cancel());
  // What was just sent must not come back as a "suggestion" from a stale screen read.
  let lastSent = "";
  const ghost = () => { const g = (!draft() && hint().suggestion) || ""; return g && g !== lastSent ? g : ""; };
  // Permission mode from the CLI's status line; the chip in the header cycles it (Shift+Tab in the CLI).
  const mode = createMemo(() => cliMode(hint().status));
  const cycleMode = () => api.sendKeys(p.paneId, ["shift+tab"]).then(() => setTimeout(readHint, 400), (e) => setError(String(e)));
  // Headless: the session's own mode and model, changed through `deck.agent.set`.
  const setAgent = (params: Record<string, unknown>) => api.agentCall(p.paneId, "set", params).catch((e) => setError(String(e)));
  const agentMode = () => live()?.permissionMode ?? "default";
  const [modelMenu, setModelMenu] = createSignal(false);
  const [modeMenu, setModeMenu] = createSignal(false);
  const interrupt = () => api.agentCall(p.paneId, "interrupt").catch((e) => setError(String(e)));
  const answer = async (requestId: string, response: Record<string, unknown>, then?: "acceptEdits") => {
    try {
      await api.agentCall(p.paneId, "respond", { request_id: requestId, response });
      if (then) await setAgent({ permission_mode: then });
    } catch (e) {
      if (!String(e).includes("already_answered") && !String(e).includes("is not open")) setError(String(e));
    }
  };
  // The reply being written, until the transcript has it.
  const streaming = createMemo(() => {
    const text = (live()?.streaming ?? "").trim();
    if (!text || !loaded()) return "";
    const items = chat().items;
    for (let i = items.length - 1; i >= 0; i--) {
      const it = items[i];
      if (it.kind === "user") break;
      if (it.kind === "assistant" && it.text.trim().startsWith(text.slice(0, 200))) return "";
    }
    return text;
  });
  const notes = createMemo(() => (live()?.notes ?? []).slice(-3));
  const takeGhost = () => { const g = ghost(); if (!g) return false; setDraft(g); queueMicrotask(grow); return true; };

  // Background panel: the transcript's full list of running agents/shells
  // (the CLI shows a few and "↓ N more"); the CLI line that mentions the same
  // task lends its tail (elapsed · tokens) when there is one.
  const background = createMemo(() => {
    const tasks = backgroundTasks(chat().items);
    const cli = hint().below;
    return tasks.map((t) => {
      const key = t.label.slice(0, 24).toLowerCase();
      const line = key ? cli.find((l) => l.toLowerCase().includes(key)) : undefined;
      const tail = line ? line.slice(line.toLowerCase().indexOf(key) + key.length).replace(/^[^·]*·\s*/, "") : "";
      const d = Math.max(0, Math.round((now() - t.since) / 1000));
      const el = d >= 3600 ? `${Math.floor(d / 3600)}h ${Math.floor((d % 3600) / 60)}m` : d >= 60 ? `${Math.floor(d / 60)}m ${d % 60}s` : `${d}s`;
      return { ...t, tail: tail || el };
    });
  });

  // Full-size view of a thumbnail; click or Esc closes.
  const onZoomKey = (e: KeyboardEvent) => { if (e.key === "Escape" && zoom()) { e.stopPropagation(); setZoom(null); } };
  onMount(() => { window.addEventListener("keydown", onZoomKey, true); onCleanup(() => window.removeEventListener("keydown", onZoomKey, true)); });

  return (
    <div class="term chat" onContextMenu={(e) => e.preventDefault()}>
      <Show when={zoom()}>{(src) => <div class="veil zoom" onClick={() => setZoom(null)}><img src={src()} /></div>}</Show>
      <div class="th">
        <span>◆ {header().title}</span><span class="micro">{header().where}</span>
        <span class={`micro right stat-${status()}`}>{status().toUpperCase()}{chat().pendingTools ? ` · ${chat().pendingTools} TOOL${chat().pendingTools > 1 ? "S" : ""} RUNNING` : ""}</span>
        <Show when={!headless() && hint().status}><button class={`mode mode-${mode().mode}`} onClick={cycleMode} title="Claude Code permission mode · click = Shift+Tab (cycle)">⏵⏵ {MODE_LABEL[mode().mode]}</button></Show>
        <Show when={headless()}>
          <button class={`mode mode-${({ default: "default", acceptEdits: "accept-edits", plan: "plan", auto: "accept-edits", dontAsk: "plan", bypassPermissions: "bypass" } as Record<string, string>)[agentMode()] ?? "default"}`}
            onClick={() => { setModelMenu(false); setModeMenu(!modeMenu()); }} title="permission mode of this session">⏵⏵ {MODE_TITLE[agentMode()] ?? agentMode().toUpperCase()} ▾</button>
          <button class="abtn model-pick" onClick={() => { setModeMenu(false); setModelMenu(!modelMenu()); }} title="model of this session">{(live()?.model ?? "MODEL").toUpperCase()} ▾</button>
          <Show when={status() !== "idle"}><button class="abtn" onClick={interrupt} title="stop the agent (Esc)">■ STOP</button></Show>
        </Show>
        <Show when={p.onTerminal}>{(t) => <button class="abtn" title="raw terminal (prompts, menus, the CLI itself)" onClick={t()}>TERM</button>}</Show>
      </div>
      <Show when={headless() && modeMenu()}>
        <div class="model-pop mode-pop" onMouseLeave={() => setModeMenu(false)}>
          <For each={PERMISSION_MODES}>{(m) => (
            <div class={`slash-it ${m === agentMode() ? "cur" : ""}`} onClick={() => { setModeMenu(false); void setAgent({ permission_mode: m }); }}>
              <span class="sn">{MODE_TITLE[m]}</span><span class="sd">{MODE_HINT[m]}</span>
            </div>
          )}</For>
        </div>
      </Show>
      <Show when={headless() && modelMenu()}>
        <div class="model-pop" onMouseLeave={() => setModelMenu(false)}>
          <For each={live()?.models ?? []} fallback={<div class="micro">the session has not listed its models yet</div>}>{(m) => (
            <div class="slash-it" onClick={() => { setModelMenu(false); void setAgent({ model: m.value === "default" ? null : m.value }); }}>
              <span class="sn">{m.displayName}</span><span class="sd">{m.description}</span>
            </div>
          )}</For>
        </div>
      </Show>
      <div class="pane-body">
        <div class="chat-log" ref={scroller} onScroll={onScroll}>
          <Show when={truncated()}><div class="chat-note">· older messages are in the transcript, not loaded ·</div></Show>
          <Show when={!chat().items.length && !error() && !streaming() && !live()?.asks.length}><div class="chat-note chat-start">
            <div>· new session — nothing said yet ·</div>
            <Show when={headless()}><div class="micro">Claude works on <b>{p.machine || "this server"}</b> in <b>{pane()?.cwd || "its home folder"}</b>: ask it anything there. To work in a repository, open it with + next to SESSIONS.</div></Show>
          </div></Show>
          <For each={chat().items}>{(it, i) => (
            <>
              <Item it={it} />
              <Show when={footers().get(i())}>{(f) => (
                <Show when={!f().open || status() !== "working"}>
                  <div class="turn-foot">✻ {fmtDuration(f().ms)} · done {clock(f().end)}</div>
                </Show>
              )}</Show>
            </>
          )}</For>
          <Show when={streaming()}>{(text) => (
            <div class="msg agent live"><div class="bub">
              <div class="who">CLAUDE <span class="when">{status() === "idle" ? "" : "writing…"}</span></div>
              <div class="mdtext" innerHTML={md(text())} />
            </div></div>
          )}</Show>
          <Show when={since() !== null}>
            <div class="chat-working"><Think size={28} /> {hint().activity || `${verb()}…`} <span class="mono">({elapsed()}{chat().pendingTools ? ` · ${chat().pendingTools} tool${chat().pendingTools > 1 ? "s" : ""} running` : ""})</span></div>
          </Show>
          <Show when={headless()}>
            {p.signIn?.()}
            <For each={live()?.asks ?? []}>{(a) => <AskCard ask={a} md={md} answer={(r, then) => answer(a.requestId, r, then)} />}</For>
            <Show when={live()?.host.state === "exited"}>
              <div class="chat-note err">· Claude stopped ({live()?.host.detail ?? "exited"}) · starting it again ·</div>
            </Show>
            <For each={notes()}>{(n) => <div class={`chat-note ${n.kind === "error" ? "err" : ""}`}>{n.text}</div>}</For>
            <Show when={live()?.linkError}>{(e) => <div class="chat-note err">· no live link to the session: {e()} · reconnecting ·</div>}</Show>
            <Show when={!live()}><div class="chat-note">· connecting to the session ·</div></Show>
          </Show>
          <Show when={!headless() && menu()} fallback={
            <Show when={!headless() && status() === "blocked"}>
              <div class="chat-blocked" onClick={() => p.onTerminal?.()}>▲ THE AGENT IS WAITING FOR YOU — a prompt or a question is on the terminal · click to open TERM</div>
            </Show>
          }>
            {(m) => (
              <div class="chat-menu">
                <div class="cm-title">▲ {m().title || "THE AGENT ASKS"}</div>
                <For each={m().options}>{(o, i) => (
                  <button class={`cm-opt ${i() === m().current ? "cur" : ""}`} disabled={choosing()} onClick={() => choose(i())}>
                    <span class="n">{o.n}</span><span class="lb">{o.label}</span>
                  </button>
                )}</For>
                <div class="cm-foot"><span>click an option · it is chosen in the CLI</span><span class="sp" /><button class="abtn" onClick={() => p.onTerminal?.()}>TERM</button></div>
              </div>
            )}
          </Show>
          <Show when={error()}>{(e) => <div class="chat-note err">{e()}</div>}</Show>
        </div>
        <Show when={pasted().length}>
          <div class="chat-pending">
            <For each={pasted()}>{(src, i) => (
              <div class="pend">
                <img class="thumb" src={src} onClick={() => setZoom(src)} title="click to view" />
                <Show when={i() === pasted().length - 1}><button class="x" title="remove (Backspace)" onClick={dropLastImage}>×</button></Show>
              </div>
            )}</For>
          </div>
        </Show>
        <div class="chat-input">
          <span class="chat-prompt">❯</span>
          <Show when={!headless() && pasted().length}><span class="chat-chip" title={headless() ? "sent with the message" : "in the agent's input; Enter sends"}>{pasted().map((_, i) => `[Image #${i + 1}]`).join(" ")}</span></Show>
          <div class="chat-field">
            <Show when={slashOpen()}>
              <div class="slash-pop">
                <For each={slash()}>{(it, i) => (
                  <div class={`slash-it ${i() === slashAt() ? "cur" : ""}`} onMouseDown={(e) => { e.preventDefault(); complete(it.name); }} onMouseMove={() => setSlashAt(i())}>
                    <span class="sn">/{it.name}<Show when={it.args}>{(a) => <i class="sa"> {a()}</i>}</Show></span>
                    <span class={`sk ${it.kind}`}>{it.kind === "builtin" ? "cli" : `${it.kind} · ${it.source}`}</span>
                    <span class="sd">{it.description}</span>
                  </div>
                )}</For>
              </div>
            </Show>
            <Show when={!slashOpen() && typed()}>{(t) => (
              <div class="slash-hint">
                <span class="sn">/{t().item.name}</span>
                <Show when={t().item.args}>{(a) => <span class={`sa ${t().args ? "" : "want"}`}>{a()}</span>}</Show>
                <span class={`sk ${t().item.kind}`}>{t().item.kind === "builtin" ? "cli" : `${t().item.kind} · ${t().item.source}`}</span>
                <span class="sd">{t().item.description}</span>
              </div>
            )}</Show>
            <Show when={ghost()}>{(g) => <div class="ghost">{g()} <kbd onClick={takeGhost} title="take the suggestion (or press Tab)">TAB</kbd></div>}</Show>
            <div class="chat-hl" ref={hl} aria-hidden="true"><For each={codeSpans(draft())}>{(s) => (s.kind === "text" ? s.text : <mark>{s.text}</mark>)}</For>{"\n"}</div>
            <textarea ref={input} rows={1} value={draft()} placeholder={ghost() ? "" : "message"}
              onInput={(e) => { setDraft(e.currentTarget.value); histAt = -1; grow(); }} onScroll={syncHl} onKeyDown={onKey} onPaste={onPaste} spellcheck={false} />
          </div>
        </div>
        <div class="chat-hints">
          <span><b>Enter</b> send · <b>Shift+Enter</b> newline · <b>↑↓</b> history · <Show when={!headless()}><b>Tab</b> take suggestion · </Show><b>Esc</b> {headless() ? "stop" : "interrupt"} · <b>Ctrl+V</b> image</span>
          <span class="sp" />
          <Show when={mode().rest}>{(st) => <span class="cli-status" title="the CLI's status line">{st()}</span>}</Show>
        </div>
        <Show when={background().length || hint().below.length}>
          <div class="chat-panel" title="background agents and shells of this session (from the transcript; tokens from the CLI when shown)">
            <Show when={background().length} fallback={<For each={hint().below}>{(l) => <div>{l}</div>}</For>}>
              <div class="bg-head">● main · {background().length} in background</div>
              <For each={background()}>{(t) => (
                <div class="bg-row"><span class={`bg-k ${t.kind}`}>{t.kind === "agent" ? "○" : "▸"}</span><span class="bg-t">{t.detail ?? t.kind}</span><span class="bg-l">{t.label}</span><span class="bg-tail">{t.tail}</span></div>
              )}</For>
            </Show>
          </div>
        </Show>
      </div>
    </div>
  );
}

/** The user's own text: plain, except `inline code` and ```fenced``` blocks, shown like the agent's. */
function UserText(p: { text: string }) {
  // A block is its own line, so the newlines around it would only add blank lines.
  const spans = () => codeSpans(p.text).map((s, i, all) => (s.kind !== "text" ? s : {
    ...s, text: s.text.replace(all[i - 1]?.kind === "block" ? /^\n/ : /^(?!)/, "").replace(all[i + 1]?.kind === "block" ? /\n$/ : /$(?!)/, ""),
  }));
  return <For each={spans()}>{(s) => (s.kind === "text" ? s.text : s.kind === "code" ? <code>{codeBody(s)}</code> : <pre class="ublock"><code>{codeBody(s)}</code></pre>)}</For>;
}

function Item(p: { it: ChatItem }) {
  const it = p.it;
  switch (it.kind) {
    case "user":
      return (
        <div class="msg user"><div class="bub">
          <div class="who">YOU <span class="when">{clock(it.ts)}</span></div>
          <Show when={it.text}><div class="utext"><UserText text={it.text} /></div></Show>
          <Show when={it.images.length}><div class="chat-thumbs"><For each={it.images}>{(src) => (src ? <img class="thumb" src={src} onClick={() => setZoom(src)} title="click to view" /> : <span class="thumb none">image</span>)}</For></div></Show>
        </div></div>
      );
    case "assistant":
      return (
        <div class={`msg agent${it.summary ? " summary" : ""}`}><div class="bub">
          <div class="who">CLAUDE <span class="when">{clock(it.ts)}</span><Show when={it.summary}><span class="tag">· summary</span></Show></div>
          <div class="mdtext" innerHTML={md(it.text)} />
        </div></div>
      );
    case "compact":
      return <Compact text={it.text} ts={it.ts} />;
    case "tools":
      return <ToolRun calls={it.calls} />;
    case "system":
      return <div class="msg sys"><span class="who">{it.subtype.replace(/_/g, " ").toUpperCase()}</span> {it.text}</div>;
  }
}

/** Context compaction: the injected summary, folded to one line until clicked. */
function Compact(p: { text: string; ts: number }) {
  const [open, setOpen] = createSignal(false);
  return (
    <div class={`compact ${open() ? "open" : ""}`}>
      <div class="compact-head" onClick={() => setOpen(!open())}>
        <span class="glyph">▞▚</span>
        <b>MEMORY DEFRAG</b>
        <span class="sub">context compacted · continuation summary loaded</span>
        <span class="when">{clock(p.ts)}</span>
        <span class="caret">{open() ? "▴" : "▾"}</span>
      </div>
      <Show when={open()}><pre class="compact-body">{p.text}</pre></Show>
    </div>
  );
}

/** A run of tool calls: two or more fold into one line (`▸ 31 STEPS · Bash ×24 · Write ×7`),
 *  keeping the call still running visible; click to open the list. */
const FOLD_AT = 1;
function ToolRun(p: { calls: ToolCall[] }) {
  const [open, setOpen] = createSignal(false);
  const folded = () => p.calls.length > FOLD_AT && !open();
  const counts = () => {
    const m = new Map<string, number>();
    for (const c of p.calls) m.set(c.name, (m.get(c.name) ?? 0) + 1);
    return [...m.entries()].sort((a, b) => b[1] - a[1]).map(([n, k]) => (k > 1 ? `${n} ×${k}` : n)).join(" · ");
  };
  const running = () => p.calls.filter((c) => c.result === undefined);
  const errors = () => p.calls.filter((c) => c.isError).length;
  return (
    <div class="msg tools">
      <Show when={p.calls.length > FOLD_AT}>
        <div class={`fold ${running().length ? "run" : errors() ? "err" : "ok"}`} onClick={() => setOpen(!open())}>
          <span class={`mk chev ${open() ? "open" : ""}`} /> <b>{p.calls.length} STEPS</b> <span class="sum">{counts()}{errors() ? ` · ${errors()} failed` : ""}</span><span class="when">{clock(p.calls[0].ts)}</span>
        </div>
      </Show>
      <Show when={folded()} fallback={<For each={p.calls}>{(c) => <Tool c={c} />}</For>}>
        <For each={running().slice(-1)}>{(c) => <Tool c={c} />}</For>
      </Show>
    </div>
  );
}

function Tool(p: { c: ToolCall }) {
  const [open, setOpen] = createSignal(false);
  const c = p.c;
  const state = () => (c.result === undefined ? "run" : c.isError ? "err" : "ok");
  const result = () => (c.result ?? "").slice(0, 20_000);
  return (
    <div class={`tool ${state()}`}>
      <div class="tool-head" onClick={() => setOpen(!open())}>
        <span class="mk state" /> <b>{c.name}</b> <span class="sum">{c.summary}</span>
        <span class="when">{clock(c.ts)}</span><span class="micro right">{open() ? "▴" : "▾"}</span>
      </div>
      <Show when={open()}>
        <pre class="tool-io">{JSON.stringify(c.input, null, 2)}</pre>
        <Show when={c.result !== undefined}><pre class="tool-io out">{result() || "(empty)"}</pre></Show>
      </Show>
    </div>
  );
}
