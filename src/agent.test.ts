import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import {
  agentStatus, allow, answerQuestions, applyMsg, askSummary, deny, emptyAgent, foldAgentLine, isHeadlessAgent, nextMode, questionsOf,
  type AgentLive,
} from "./agent";

// The recorded Claude Code session the server's tests use too
// (AskUserQuestion, then a Write that asked for permission).
const FIXTURE = readFileSync(fileURLToPath(new URL("../server/crates/deck-agent/fixtures/ask-then-write.jsonl", import.meta.url)), "utf8");
const msgs = FIXTURE.trim().split("\n").map((l) => JSON.parse(l) as Record<string, unknown>);

/** The fixture as a subscriber sees it: numbered events, a `deck_resolved` after each request. */
function stream(epoch = "e1"): unknown[] {
  const out: unknown[] = [{ id: "s", result: { pane_id: "p", epoch, connected: true, status: "idle", session_id: null } }];
  let seq = 0;
  for (const m of msgs) {
    out.push({ event: "deck.agent.event", data: { pane_id: "p", epoch, seq: ++seq, msg: m } });
    if (m.type === "control_request") out.push({ event: "deck.agent.event", data: { pane_id: "p", epoch, seq: ++seq, msg: { type: "deck_resolved", request_id: m.request_id } } });
  }
  return out;
}

const fold = (lines: unknown[], from?: AgentLive) => lines.reduce<AgentLive | undefined>((a, l) => foldAgentLine(a, l), from)!;

describe("foldAgentLine over a real session", () => {
  it("follows status, asks, streaming text and the session's meta", () => {
    const lines = stream();
    const seen: string[] = [];
    let a: AgentLive | undefined;
    let maxStreaming = "";
    for (const l of lines) {
      a = foldAgentLine(a, l);
      const st = agentStatus(a);
      if (seen[seen.length - 1] !== st) seen.push(st);
      if (a.streaming.length > maxStreaming.length) maxStreaming = a.streaming;
    }
    expect(seen).toEqual(["idle", "working", "blocked", "working", "blocked", "working", "idle"]);
    expect(a!.sessionId).toBe("60eef6dd-b9b4-4735-8aa2-174874c082f1");
    expect(a!.model).toBeTruthy();
    expect(a!.permissionMode).toBe("default");
    expect(a!.commands).toEqual([{ name: "review", description: "review a pull request", argumentHint: "" }]);
    expect(a!.models[0].value).toBe("default");
    expect(a!.asks).toEqual([]);
    // The last reply's text stays (the chat hides it once the transcript has it).
    const lastText = [...msgs].reverse().find((m) => m.type === "assistant" && JSON.stringify(m).includes('"type":"text"')) as { message: { content: { text?: string }[] } };
    expect(a!.streaming).toBe(lastText.message.content.find((b) => b.text)!.text);
    expect(maxStreaming.length).toBeGreaterThan(0);
  });

  it("holds the open request with what the card needs", () => {
    const lines = stream();
    const at = lines.findIndex((l) => (l as { data?: { msg?: { type?: string } } }).data?.msg?.type === "control_request");
    const a = fold(lines.slice(0, at + 1));
    expect(agentStatus(a)).toBe("blocked");
    expect(a.asks).toHaveLength(1);
    expect(a.asks[0].tool).toBe("AskUserQuestion");
    const qs = questionsOf(a.asks[0]);
    expect(qs[0].options.map((o) => o.label)).toEqual(["a.txt", "b.txt"]);
    const r = answerQuestions(a.asks[0], { [qs[0].question]: ["b.txt"] });
    expect(r.behavior).toBe("allow");
    expect((r.updatedInput as { answers: Record<string, string> }).answers).toEqual({ [qs[0].question]: "b.txt" });
    expect((r.updatedInput as { questions: unknown[] }).questions).toBeTruthy();
  });

  it("skips replayed events and starts over on a new epoch", () => {
    const lines = stream();
    const once = fold(lines);
    const twice = fold(lines.slice(1), once); // a reconnect replays the same epoch
    expect(twice).toEqual(once);
    const fresh = foldAgentLine(once, { event: "deck.agent.reset", data: { pane_id: "p", epoch: "e2" } });
    expect(fresh.epoch).toBe("e2");
    expect(fresh.commands).toEqual([]);
    expect(fresh.connected).toBe(true);
  });
});

describe("applyMsg", () => {
  it("settings, host state, stderr and failed results", () => {
    let a = applyMsg(emptyAgent("e"), { type: "deck_settings", permission_mode: "plan", model: "sonnet" });
    expect([a.permissionMode, a.model]).toEqual(["plan", "sonnet"]);
    a = applyMsg(a, { type: "deck_host", state: "running" });
    expect(a.host.state).toBe("running");
    a = applyMsg(a, { type: "user", message: { content: "hi" } });
    expect(agentStatus(a)).toBe("working");
    a = applyMsg(a, { type: "deck_host", state: "log", detail: "Invalid API key" });
    a = applyMsg(a, { type: "result", subtype: "success", is_error: true, result: "Not logged in · Please run /login" });
    expect(agentStatus(a)).toBe("idle");
    expect(a.notes.map((n) => n.kind)).toEqual(["log", "error"]);
    a = applyMsg(a, { type: "deck_host", state: "exited", detail: "code 1" });
    expect(a.host).toEqual({ state: "exited", detail: "code 1" });
  });

  it("a tool result is not a new turn; a cancel closes the ask", () => {
    let a = applyMsg(emptyAgent(), { type: "user", message: { content: [{ type: "tool_result", tool_use_id: "t" }] } });
    expect(a.busy).toBe(false);
    a = applyMsg(a, { type: "control_request", request_id: "q", request: { subtype: "can_use_tool", tool_name: "Bash", input: { command: "ls" } } });
    expect(agentStatus(a)).toBe("blocked");
    a = applyMsg(a, { type: "control_cancel_request", request_id: "q" });
    expect(a.asks).toEqual([]);
  });

  it("the link note", () => {
    const a = foldAgentLine(emptyAgent("e"), { event: "deck.agent.link", data: { error: "offline" } });
    expect(a.linkError).toBe("offline");
    const b = foldAgentLine(a, { id: "s", result: { epoch: "e", connected: true } });
    expect(b.linkError).toBeNull();
  });
});

describe("answers and labels", () => {
  const ask = { requestId: "r", tool: "Write", input: { file_path: "/a.txt", content: "hi" }, description: "a.txt", suggestions: [{ type: "setMode", mode: "acceptEdits", destination: "session" }], toolUseId: "t" };
  it("allow / deny / remember", () => {
    expect(allow(ask)).toEqual({ behavior: "allow", updatedInput: ask.input });
    expect(allow(ask, ask.input, true).updatedPermissions).toEqual(ask.suggestions);
    expect(deny("no").behavior).toBe("deny");
  });
  it("card summaries", () => {
    expect(askSummary(ask)).toEqual({ head: "write /a.txt", body: "hi" });
    expect(askSummary({ ...ask, tool: "Bash", input: { command: "ls -la", description: "List files" } })).toEqual({ head: "List files", body: "ls -la" });
    expect(askSummary({ ...ask, tool: "ExitPlanMode", input: { plan: "1. do it" } }).body).toBe("1. do it");
    expect(askSummary({ ...ask, tool: "mcp__x__y", input: {}, description: "" }).head).toBe("mcp__x__y");
  });
  it("modes and agent label", () => {
    expect(nextMode("default")).toBe("acceptEdits");
    expect(nextMode("plan")).toBe("default");
    expect(nextMode(null)).toBe("acceptEdits");
    expect(isHeadlessAgent("deck-claude")).toBe(true);
    expect(isHeadlessAgent("claude")).toBe(false);
  });
});

describe("sign-in needed", () => {
  const init = (tokenSource: string) => ({ type: "control_response", response: { subtype: "success", request_id: "deck-init", response: { account: { apiProvider: "firstParty", tokenSource } } } });
  it("follows Claude Code's own verdict", () => {
    let a = applyMsg(emptyAgent("e"), init("none"));
    expect(a.authRequired).toBe(true);
    a = applyMsg(a, { type: "result", subtype: "success", is_error: true, result: "Authentication required · Sign in again to continue" });
    expect(a.authRequired).toBe(true);
    a = applyMsg(applyMsg(emptyAgent("e"), init("claude.ai")), { type: "assistant", error: "authentication_failed", message: { content: [] } });
    expect(a.authRequired).toBe(true);
    a = applyMsg(a, { type: "result", subtype: "success", is_error: false, result: "hi" });
    expect(a.authRequired).toBe(false);
    expect(applyMsg(emptyAgent("e"), init("claude.ai")).authRequired).toBe(false);
  });
  it("the host's running note names the session", () => {
    const a = applyMsg(emptyAgent("e"), { type: "deck_host", state: "running", detail: null, session_id: "s-9" });
    expect([a.sessionId, a.host.state]).toEqual(["s-9", "running"]);
  });
});
