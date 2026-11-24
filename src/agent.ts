/**
 * Live state of a headless Claude Code session (deck-server's
 * `deck-server agent`, docs/adr/0003-headless-claude.md), folded from the
 * lines of its `deck.agent.subscribe` stream. Pure: the Rust side hands
 * over each line, this module decides what it means.
 *
 * The conversation itself still comes from the transcript (`transcript.ts`);
 * this is what the transcript does not carry: the text being written right
 * now, the questions waiting for an answer (permission, AskUserQuestion,
 * plan approval), the status, model and permission mode, the session's
 * slash commands and models, and what the `claude` process said on stderr.
 */

export type AgentStatus = "idle" | "working" | "blocked";

/** A `can_use_tool` request from Claude Code, waiting for an answer. */
export interface Ask {
  requestId: string;
  tool: string;
  input: Record<string, unknown>;
  /** Claude Code's one-line summary (e.g. the file name). */
  description: string;
  /** Rules Claude Code offers to remember ("always allow …"), passed back as `updatedPermissions`. */
  suggestions: unknown[];
  toolUseId: string;
}

export interface SlashCommand { name: string; description: string; argumentHint: string }
export interface ModelChoice { value: string; displayName: string; description: string }
export interface Note { kind: "log" | "error"; text: string }

export interface AgentLive {
  epoch: string;
  seq: number;
  /** The host is connected to its server (it relays `claude`). */
  connected: boolean;
  /** Deck's own link to the server's stream is down (message), or null. */
  linkError: string | null;
  busy: boolean;
  asks: Ask[];
  sessionId: string | null;
  model: string | null;
  permissionMode: string | null;
  /** Text of the reply being written, until its message ends. */
  streaming: string;
  /** The model is thinking (a thinking block is open). */
  thinking: boolean;
  commands: SlashCommand[];
  models: ModelChoice[];
  /** `claude` process: running, exited (detail = exit code), or not seen yet. */
  host: { state: "running" | "exited" | ""; detail: string | null };
  /** Recent stderr lines and failed results, newest last. */
  notes: Note[];
  /** `claude` has no usable login on its machine (no token, or the API refused it): the chat offers sign-in. */
  authRequired: boolean;
}

export const MAX_NOTES = 30;

export function emptyAgent(epoch = ""): AgentLive {
  return {
    epoch, seq: 0, connected: false, linkError: null, busy: false, asks: [], sessionId: null, model: null,
    permissionMode: null, streaming: "", thinking: false, commands: [], models: [], host: { state: "", detail: null }, notes: [],
    authRequired: false,
  };
}

export function agentStatus(a: AgentLive): AgentStatus {
  return a.asks.length ? "blocked" : a.busy ? "working" : "idle";
}

const str = (v: unknown): string => (typeof v === "string" ? v : "");
const rec = (v: unknown): Record<string, unknown> | null => (typeof v === "object" && v !== null && !Array.isArray(v) ? (v as Record<string, unknown>) : null);

const note = (a: AgentLive, n: Note): Note[] => [...a.notes, n].slice(-MAX_NOTES);

/** A user turn echoed back (not a tool result). */
function isUserTurn(msg: Record<string, unknown>): boolean {
  const content = rec(msg.message)?.content;
  if (typeof content === "string") return true;
  return Array.isArray(content) && !content.some((b) => rec(b)?.type === "tool_result");
}

/** One event (`msg` of a `deck.agent.event`) applied to the state. */
export function applyMsg(a: AgentLive, msg: Record<string, unknown>): AgentLive {
  const type = str(msg.type);
  const sid = str(msg.session_id);
  let next: AgentLive = sid && sid !== a.sessionId ? { ...a, sessionId: sid } : a;
  switch (type) {
    case "system":
      if (msg.subtype === "init") {
        next = { ...next, model: str(msg.model) || next.model, permissionMode: str(msg.permissionMode) || next.permissionMode };
      }
      return next;
    case "control_response": {
      const response = rec(msg.response);
      if (response?.request_id !== "deck-init" || response.subtype !== "success") return next;
      const body = rec(response.response);
      const commands = Array.isArray(body?.commands)
        ? body.commands.map(rec).filter((c): c is Record<string, unknown> => !!c && !!str(c.name))
          .map((c) => ({ name: str(c.name), description: str(c.description), argumentHint: str(c.argumentHint) }))
        : next.commands;
      // Claude Code says which credential it found; "none" = not signed in.
      // Signed in, the account carries the email and no tokenSource.
      const account = rec(body?.account);
      if (account) next = { ...next, authRequired: str(account.tokenSource) === "none" && !str(account.email) && !str(account.apiKeySource) };
      const models = Array.isArray(body?.models)
        ? body.models.map(rec).filter((m): m is Record<string, unknown> => !!m && !!str(m.value))
          .map((m) => ({ value: str(m.value), displayName: str(m.displayName) || str(m.value), description: str(m.description) }))
        : next.models;
      return { ...next, commands, models };
    }
    case "control_request": {
      const req = rec(msg.request);
      const id = str(msg.request_id);
      if (req?.subtype !== "can_use_tool" || !id || next.asks.some((x) => x.requestId === id)) return next;
      const ask: Ask = {
        requestId: id, tool: str(req.tool_name), input: rec(req.input) ?? {}, description: str(req.description),
        suggestions: Array.isArray(req.permission_suggestions) ? req.permission_suggestions : [], toolUseId: str(req.tool_use_id),
      };
      return { ...next, asks: [...next.asks, ask] };
    }
    case "deck_resolved":
    case "control_cancel_request": {
      const id = str(msg.request_id);
      return next.asks.some((x) => x.requestId === id) ? { ...next, asks: next.asks.filter((x) => x.requestId !== id) } : next;
    }
    case "stream_event": {
      const ev = rec(msg.event);
      next = next.busy ? next : { ...next, busy: true };
      switch (str(ev?.type)) {
        case "message_start":
          return { ...next, streaming: "", thinking: false };
        case "content_block_start": {
          const kind = str(rec(ev?.content_block)?.type);
          if (kind === "thinking") return { ...next, thinking: true };
          if (kind === "text") return { ...next, thinking: false, streaming: next.streaming ? `${next.streaming}\n\n` : "" };
          return { ...next, thinking: false };
        }
        case "content_block_delta": {
          const d = rec(ev?.delta);
          if (d?.type === "text_delta") return { ...next, streaming: next.streaming + str(d.text) };
          return next;
        }
        case "message_stop":
          return { ...next, thinking: false };
        default:
          return next;
      }
    }
    case "assistant":
      if (msg.error === "authentication_failed") return { ...next, busy: true, authRequired: true };
      return next.busy ? next : { ...next, busy: true };
    case "user":
      return isUserTurn(msg) ? { ...next, busy: true, streaming: "" } : next;
    case "result": {
      const failed = msg.is_error === true || (str(msg.subtype) !== "" && str(msg.subtype) !== "success");
      const text = str(msg.result) || str(msg.subtype);
      // The written text stays until the transcript shows it (the chat hides it then).
      return {
        ...next, busy: false, asks: [], thinking: false,
        notes: failed && text ? note(next, { kind: "error", text }) : next.notes,
        authRequired: failed ? next.authRequired : false,
      };
    }
    case "deck_settings": {
      const out = { ...next };
      if ("model" in msg) out.model = typeof msg.model === "string" ? msg.model : null;
      if (typeof msg.permission_mode === "string") out.permissionMode = msg.permission_mode;
      return out;
    }
    case "deck_host": {
      const state = str(msg.state);
      const detail = typeof msg.detail === "string" ? msg.detail : null;
      if (state === "log") return detail ? { ...next, notes: note(next, { kind: "log", text: detail }) } : next;
      // A fresh `claude` (e.g. restarted after a sign-in) has not failed yet; its init says again if it has no login.
      if (state === "running") return { ...next, host: { state: "running", detail: null }, authRequired: false };
      if (state === "exited") return { ...next, host: { state: "exited", detail }, busy: false, asks: [], streaming: "", thinking: false };
      return next;
    }
    default:
      return next;
  }
}

/**
 * One line of the subscription (or Deck's own `deck.agent.link` note) applied
 * to the pane's state. `undefined` state = not seen yet.
 */
export function foldAgentLine(prev: AgentLive | undefined, line: unknown): AgentLive {
  const a = prev ?? emptyAgent();
  const o = rec(line);
  if (!o) return a;
  const result = rec(o.result);
  if (result) {
    // The subscription's head: the server's summary of the pane.
    const epoch = str(result.epoch);
    const base = epoch && epoch !== a.epoch ? emptyAgent(epoch) : a;
    return {
      ...base,
      linkError: null,
      connected: result.connected === true,
      sessionId: str(result.session_id) || base.sessionId,
      model: str(result.model) || base.model,
      permissionMode: str(result.permission_mode) || base.permissionMode,
    };
  }
  const data = rec(o.data);
  switch (o.event) {
    case "deck.agent.reset":
      return { ...emptyAgent(str(data?.epoch)), connected: true };
    case "deck.agent.link":
      return { ...a, linkError: str(data?.error) || "link down" };
    case "deck.agent.event": {
      const msg = rec(data?.msg);
      const seq = typeof data?.seq === "number" ? data.seq : 0;
      const epoch = str(data?.epoch);
      if (!msg) return a;
      // Replays after a reconnect can repeat what was already applied.
      if (epoch === a.epoch && seq <= a.seq) return a;
      const base = epoch && epoch !== a.epoch ? emptyAgent(epoch) : a;
      return { ...applyMsg(base, msg), seq, epoch: epoch || base.epoch, connected: true, linkError: null };
    }
    default:
      return a;
  }
}

/** deck-server panes run `deck-server agent` and report this agent label. */
export const HEADLESS_AGENT = "deck-claude";
export const isHeadlessAgent = (agent: string | null | undefined) => (agent ?? "").toLowerCase() === HEADLESS_AGENT;

export const PERMISSION_MODES = ["default", "acceptEdits", "plan", "bypassPermissions"] as const;
export type PermissionMode = (typeof PERMISSION_MODES)[number];
export const MODE_TITLE: Record<string, string> = { default: "DEFAULT", acceptEdits: "ACCEPT EDITS", plan: "PLAN", bypassPermissions: "BYPASS" };
/** The next mode in the CLI's own Shift+Tab order. */
export function nextMode(mode: string | null): PermissionMode {
  const order: PermissionMode[] = ["default", "acceptEdits", "plan"];
  const i = order.indexOf((mode ?? "default") as PermissionMode);
  return order[(i + 1) % order.length];
}

// ---- answers ----------------------------------------------------------------

/** Allow, as asked (or with an edited input, e.g. AskUserQuestion's answers). */
export function allow(ask: Ask, input: Record<string, unknown> = ask.input, remember = false): Record<string, unknown> {
  const r: Record<string, unknown> = { behavior: "allow", updatedInput: input };
  if (remember && ask.suggestions.length) r.updatedPermissions = ask.suggestions;
  return r;
}

export function deny(message = "The user declined."): Record<string, unknown> {
  return { behavior: "deny", message };
}

export interface Question { question: string; header: string; multiSelect: boolean; options: { label: string; description: string }[] }

/** AskUserQuestion's questions, as the tool input carries them. */
export function questionsOf(ask: Ask): Question[] {
  const qs = ask.input.questions;
  if (!Array.isArray(qs)) return [];
  return qs.map(rec).filter((q): q is Record<string, unknown> => !!q && !!str(q.question)).map((q) => ({
    question: str(q.question), header: str(q.header), multiSelect: q.multiSelect === true,
    options: Array.isArray(q.options) ? q.options.map(rec).filter((o): o is Record<string, unknown> => !!o).map((o) => ({ label: str(o.label), description: str(o.description) })) : [],
  }));
}

/** The answer to AskUserQuestion: the input with `answers` (question → label, several joined by ", "). */
export function answerQuestions(ask: Ask, picked: Record<string, string[]>): Record<string, unknown> {
  const answers: Record<string, string> = {};
  for (const q of questionsOf(ask)) {
    const p = (picked[q.question] ?? []).map((s) => s.trim()).filter(Boolean);
    if (p.length) answers[q.question] = p.join(", ");
  }
  return allow(ask, { ...ask.input, answers });
}

/** What a permission card shows for a tool's input: one headline and an optional body. */
export function askSummary(ask: Ask): { head: string; body: string } {
  const i = ask.input;
  switch (ask.tool) {
    case "Bash": return { head: str(i.description) || "run a command", body: str(i.command) };
    case "Write": return { head: `write ${str(i.file_path)}`, body: str(i.content).slice(0, 4000) };
    case "Edit": return { head: `edit ${str(i.file_path)}`, body: [str(i.old_string) && `- ${str(i.old_string)}`, str(i.new_string) && `+ ${str(i.new_string)}`].filter(Boolean).join("\n").slice(0, 4000) };
    case "MultiEdit": return { head: `edit ${str(i.file_path)}`, body: "" };
    case "NotebookEdit": return { head: `edit ${str(i.notebook_path)}`, body: str(i.new_source).slice(0, 4000) };
    case "WebFetch": return { head: `fetch ${str(i.url)}`, body: str(i.prompt) };
    case "WebSearch": return { head: `search “${str(i.query)}”`, body: "" };
    case "ExitPlanMode": return { head: "the plan is ready", body: str(i.plan) };
    default: return { head: ask.description || ask.tool, body: Object.keys(i).length ? JSON.stringify(i, null, 2).slice(0, 4000) : "" };
  }
}
