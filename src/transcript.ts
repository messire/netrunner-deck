/**
 * Claude Code transcript (`~/.claude/projects/<cwd>/<session>.jsonl`) → chat
 * items. Pure: the Rust side hands over raw lines, this module decides what
 * they mean. One line per event; the ones that matter for a conversation:
 *   user      message.content: string | [{text}|{image}|{tool_result}]
 *   assistant message.content: [{text}|{thinking}|{tool_use}]  (one block per line)
 *   system    subtype + content (away_summary, …)
 *   ai-title  the session's title
 * Everything else (api-request blobs, bookkeeping) is ignored. Sidechain
 * (subagent) and meta lines are ignored too.
 */
export type Block =
  | { kind: "user"; uuid: string; ts: number; text: string; images: string[] }
  | { kind: "compact"; uuid: string; ts: number; text: string }
  | { kind: "assistant"; uuid: string; ts: number; msgId: string; text: string }
  | { kind: "tool_use"; uuid: string; ts: number; msgId: string; id: string; name: string; input: unknown }
  | { kind: "tool_result"; uuid: string; ts: number; toolUseId: string; text: string; isError: boolean }
  | { kind: "system"; uuid: string; ts: number; subtype: string; text: string }
  | { kind: "title"; ts: number; text: string };

const str = (v: unknown): string => (typeof v === "string" ? v : "");
const rec = (v: unknown): Record<string, unknown> | null => (typeof v === "object" && v !== null && !Array.isArray(v) ? (v as Record<string, unknown>) : null);

function tsOf(o: Record<string, unknown>): number {
  const t = o.timestamp;
  if (typeof t === "number") return t;
  const ms = Date.parse(str(t));
  return Number.isFinite(ms) ? ms : 0;
}

/** Text of a tool_result `content`: a string, or text parts of an array. */
function resultText(content: unknown): string {
  if (typeof content === "string") return content;
  if (Array.isArray(content)) return content.map((p) => str(rec(p)?.text)).filter(Boolean).join("\n");
  return "";
}

/** A transcript image block's source as something an <img> can show ("" when unknown). */
function imageUrl(source: unknown): string {
  const src = rec(source);
  if (!src) return "";
  if (src.type === "base64" && typeof src.data === "string") return `data:${str(src.media_type) || "image/png"};base64,${src.data}`;
  if (src.type === "url") return str(src.url);
  return "";
}

/**
 * A user turn's text as the chat shows it. Slash commands are recorded as
 * `<command-name>/x</command-name><command-args>…</command-args>` (plus the
 * expanded prompt in a later, meta line): shown as `/x …`. Other tagged
 * payloads (`<local-command-stdout>`, system reminders) are not the user's words.
 */
export function userText(raw: string): string {
  const text = raw.trim();
  if (!text.startsWith("<")) return text;
  const name = /<command-name>([\s\S]*?)<\/command-name>/.exec(text)?.[1]?.trim();
  if (!name) return "";
  const args = /<command-args>([\s\S]*?)<\/command-args>/.exec(text)?.[1]?.trim() ?? "";
  return `${name.startsWith("/") ? name : `/${name}`}${args ? ` ${args}` : ""}`;
}

/** The continuation summary Claude Code injects as a user turn after compacting the context. */
const COMPACT_MARK = "This session is being continued from a previous conversation";
const isCompact = (text: string) => text.startsWith(COMPACT_MARK);

/** Parse one JSONL line into zero or more blocks. Never throws. */
export function parseLine(line: string): Block[] {
  let o: Record<string, unknown> | null;
  try { o = rec(JSON.parse(line)); } catch { return []; }
  if (!o) return [];
  if (o.isSidechain === true || o.isMeta === true) return [];
  const type = str(o.type);
  const ts = tsOf(o);
  const uuid = str(o.uuid);
  if (type === "ai-title") {
    const text = str(o.title) || str(o.content) || str(o.text);
    return text ? [{ kind: "title", ts, text }] : [];
  }
  if (type === "system") {
    const text = str(o.content);
    return text ? [{ kind: "system", uuid, ts, subtype: str(o.subtype) || "system", text }] : [];
  }
  const message = rec(o.message);
  if (!message) return [];
  const content = message.content;
  if (type === "user") {
    if (typeof content === "string") {
      const text = userText(content);
      if (text && isCompact(text)) return [{ kind: "compact", uuid, ts, text }];
      return text ? [{ kind: "user", uuid, ts, text, images: [] }] : [];
    }
    if (!Array.isArray(content)) return [];
    const out: Block[] = [];
    const texts: string[] = [];
    const images: string[] = [];
    for (const part of content) {
      const p = rec(part);
      if (!p) continue;
      if (p.type === "tool_result") out.push({ kind: "tool_result", uuid, ts, toolUseId: str(p.tool_use_id), text: resultText(p.content), isError: p.is_error === true });
      else if (p.type === "text") texts.push(str(p.text));
      else if (p.type === "image") images.push(imageUrl(p.source));
    }
    const text = userText(texts.join("\n"));
    if (text && isCompact(text) && !images.length) out.push({ kind: "compact", uuid, ts, text });
    else if (text || images.length) out.push({ kind: "user", uuid, ts, text, images });
    return out;
  }
  if (type === "assistant") {
    if (!Array.isArray(content)) return [];
    const msgId = str(message.id);
    const out: Block[] = [];
    for (const part of content) {
      const p = rec(part);
      if (!p) continue;
      if (p.type === "text") { const text = str(p.text); if (text.trim()) out.push({ kind: "assistant", uuid, ts, msgId, text }); }
      else if (p.type === "tool_use") out.push({ kind: "tool_use", uuid, ts, msgId, id: str(p.id), name: str(p.name), input: p.input });
    }
    return out;
  }
  return [];
}

// ---- chat model -------------------------------------------------------------

export interface ToolCall { id: string; name: string; summary: string; input: unknown; result?: string; isError?: boolean; ts: number }
export type ChatItem =
  | { kind: "user"; key: string; ts: number; text: string; images: string[] }
  | { kind: "compact"; key: string; ts: number; text: string }
  | { kind: "assistant"; key: string; ts: number; text: string }
  | { kind: "tools"; key: string; ts: number; calls: ToolCall[] }
  | { kind: "system"; key: string; ts: number; subtype: string; text: string };

export interface ChatState { items: ChatItem[]; title: string | null; pendingTools: number }

/** One-line description of a tool call for the collapsed card. */
export function toolSummary(name: string, input: unknown): string {
  const i = rec(input) ?? {};
  const first = (keys: string[]) => keys.map((k) => str(i[k])).find(Boolean) ?? "";
  switch (name) {
    case "Bash": return first(["description"]) || firstLine(str(i.command));
    case "Read": case "Write": case "Edit": case "NotebookEdit": return str(i.file_path);
    case "Glob": case "Grep": return `${str(i.pattern)}${i.path ? ` in ${str(i.path)}` : ""}`;
    case "WebFetch": case "WebSearch": return first(["url", "query"]);
    case "Skill": return first(["skill"]);
    case "Agent": case "Task": return first(["description", "prompt"]);
    case "AskUserQuestion": return "question for you";
    case "TodoWrite": return "todo list";
    default: {
      const firstStr = Object.values(i).find((v) => typeof v === "string" && v.trim()) as string | undefined;
      return firstStr ? firstLine(firstStr) : "";
    }
  }
}
const firstLine = (s: string) => s.split("\n")[0].slice(0, 160);

/**
 * Folds blocks into chat items incrementally: consecutive assistant text of
 * one message is one bubble; a run of tool calls (with their results, matched
 * by id, even when the result arrives much later) is one tools card; user
 * turns and system notes stand alone. `push` takes only the new blocks;
 * `state()` returns the items with every item touched since the last call
 * replaced by a fresh object (so a keyed list re-renders just those) while
 * untouched items keep their identity.
 */
export class ChatBuilder {
  private items: ChatItem[] = [];
  private byTool = new Map<string, ToolCall>();
  private owner = new Map<string, number>(); // tool id → index of its tools item
  private dirty = new Set<number>();
  private pending = new Set<string>();
  title: string | null = null;

  push(blocks: Block[]): void {
    for (const b of blocks) {
      const lastIdx = this.items.length - 1;
      const last = this.items[lastIdx];
      switch (b.kind) {
        case "title": this.title = b.text; break;
        case "user": this.items.push({ kind: "user", key: b.uuid || `u${b.ts}`, ts: b.ts, text: b.text, images: b.images }); break;
        case "compact": this.items.push({ kind: "compact", key: b.uuid || `c${b.ts}`, ts: b.ts, text: b.text }); break;
        case "system": this.items.push({ kind: "system", key: b.uuid || `s${b.ts}`, ts: b.ts, subtype: b.subtype, text: b.text }); break;
        case "assistant":
          if (last?.kind === "assistant" && last.key === b.msgId) { last.text += (last.text.endsWith("\n") ? "" : "\n\n") + b.text; this.dirty.add(lastIdx); }
          else this.items.push({ kind: "assistant", key: b.msgId || b.uuid, ts: b.ts, text: b.text });
          break;
        case "tool_use": {
          const call: ToolCall = { id: b.id, name: b.name, summary: toolSummary(b.name, b.input), input: b.input, ts: b.ts };
          this.byTool.set(b.id, call);
          this.pending.add(b.id);
          if (last?.kind === "tools") { last.calls.push(call); this.dirty.add(lastIdx); this.owner.set(b.id, lastIdx); }
          else { this.items.push({ kind: "tools", key: `t${b.uuid || b.id}`, ts: b.ts, calls: [call] }); this.owner.set(b.id, this.items.length - 1); }
          break;
        }
        case "tool_result": {
          const call = this.byTool.get(b.toolUseId);
          if (call) {
            call.result = b.text; call.isError = b.isError;
            this.pending.delete(b.toolUseId);
            const i = this.owner.get(b.toolUseId);
            if (i !== undefined) this.dirty.add(i);
          }
          break;
        }
      }
    }
  }

  /** Current chat; touched items (and their tool calls) come back as new objects. */
  state(): ChatState {
    for (const i of this.dirty) {
      const it = this.items[i];
      if (it.kind === "tools") {
        const calls = it.calls.map((c) => ({ ...c }));
        this.items[i] = { ...it, calls };
        for (const c of calls) this.byTool.set(c.id, c); // later results must land on the live copies
      } else {
        this.items[i] = { ...it };
      }
    }
    this.dirty.clear();
    return { items: [...this.items], title: this.title, pendingTools: this.pending.size };
  }
}

/** One-shot fold (tests, tools): a builder fed everything at once. */
export function buildChat(blocks: Block[]): ChatState {
  const b = new ChatBuilder();
  b.push(blocks);
  return b.state();
}

/** A background task still running: a pending `Agent`/`Task` call, or a `Bash` started with `run_in_background`. */
export interface Background { id: string; kind: "agent" | "shell"; label: string; since: number; detail?: string }

/** Every background task without a result yet, oldest first — the CLI's own panel cuts this list, the transcript does not. */
export function backgroundTasks(items: ChatItem[]): Background[] {
  const out: Background[] = [];
  for (const it of items) {
    if (it.kind !== "tools") continue;
    for (const c of it.calls) {
      if (c.result !== undefined) continue;
      const input = rec(c.input) ?? {};
      if (c.name === "Agent" || c.name === "Task") {
        out.push({ id: c.id, kind: "agent", label: str(input.description) || str(input.prompt).split("\n")[0].slice(0, 80) || c.name, since: c.ts, detail: str(input.subagent_type) || undefined });
      } else if (c.name === "Bash" && input.run_in_background === true) {
        out.push({ id: c.id, kind: "shell", label: str(input.description) || str(input.command).split("\n")[0].slice(0, 80), since: c.ts });
      }
    }
  }
  return out;
}

/** Per user turn: when it was asked, when the agent last acted, and how long that took (ms); `open` while no later turn exists. */
export interface Turn { start: number; end: number; ms: number; lastIndex: number; open: boolean }

/** Turns keyed by the index of their last item, so the view can draw a footer after it. Items before the first user message form no turn. */
export function turnFooters(items: ChatItem[]): Map<number, Turn> {
  const out = new Map<number, Turn>();
  let start = -1;
  let startTs = 0;
  let lastTs = 0;
  let last = -1;
  let acted = false;
  const close = (open: boolean) => { if (start >= 0 && acted) out.set(last, { start: startTs, end: lastTs, ms: Math.max(0, lastTs - startTs), lastIndex: last, open }); };
  items.forEach((it, i) => {
    if (it.kind === "user") { close(false); start = i; startTs = it.ts; lastTs = it.ts; last = i; acted = false; return; }
    if (it.kind === "compact") return;
    if (start < 0) return;
    acted = acted || it.kind === "assistant" || it.kind === "tools";
    last = i;
    const ts = it.kind === "tools" ? Math.max(it.ts, ...it.calls.map((c) => c.ts)) : it.ts;
    if (ts > lastTs) lastTs = ts;
  });
  close(true);
  return out;
}

/** `13s`, `2m 05s`, `1h 12m`. */
export function fmtDuration(ms: number): string {
  const s = Math.round(ms / 1000);
  if (s < 60) return `${s}s`;
  if (s < 3600) return `${Math.floor(s / 60)}m ${String(s % 60).padStart(2, "0")}s`;
  return `${Math.floor(s / 3600)}h ${String(Math.floor((s % 3600) / 60)).padStart(2, "0")}m`;
}
