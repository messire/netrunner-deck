import { describe, expect, it } from "vitest";
import { ChatBuilder, backgroundTasks, buildChat, fmtDuration, parseLine, toolSummary, turnFooters } from "./transcript";

const line = (o: unknown) => JSON.stringify(o);

describe("parseLine", () => {
  it("user prompt as a string, and as text parts with images", () => {
    expect(parseLine(line({ type: "user", uuid: "u1", timestamp: "2026-11-16T10:00:00.000Z", message: { role: "user", content: "почини X" } })))
      .toEqual([{ kind: "user", uuid: "u1", ts: Date.parse("2026-11-16T10:00:00.000Z"), text: "почини X", images: [] }]);
    const parts = parseLine(line({ type: "user", uuid: "u2", message: { content: [{ type: "text", text: "see" }, { type: "image", source: { type: "base64", media_type: "image/png", data: "AAAA" } }] } }));
    expect(parts).toMatchObject([{ kind: "user", text: "see", images: ["data:image/png;base64,AAAA"] }]);
  });
  it("tool results come from user lines, with error flag and array content", () => {
    const b = parseLine(line({ type: "user", uuid: "u3", message: { content: [{ type: "tool_result", tool_use_id: "t1", content: [{ type: "text", text: "ok" }], is_error: true }] } }));
    expect(b).toEqual([{ kind: "tool_result", uuid: "u3", ts: 0, toolUseId: "t1", text: "ok", isError: true }]);
  });
  it("assistant text and tool_use blocks; thinking is dropped", () => {
    const b = parseLine(line({ type: "assistant", uuid: "a1", message: { id: "m1", content: [{ type: "thinking", thinking: "…" }, { type: "text", text: "Готово." }, { type: "tool_use", id: "t2", name: "Bash", input: { command: "ls", description: "List" } }] } }));
    expect(b).toEqual([
      { kind: "assistant", uuid: "a1", ts: 0, msgId: "m1", text: "Готово." },
      { kind: "tool_use", uuid: "a1", ts: 0, msgId: "m1", id: "t2", name: "Bash", input: { command: "ls", description: "List" } },
    ]);
  });
  it("system notes and the session title; bookkeeping, sidechain and meta lines are ignored", () => {
    expect(parseLine(line({ type: "system", subtype: "away_summary", content: "Делаю X", uuid: "s1" }))).toMatchObject([{ kind: "system", subtype: "away_summary", text: "Делаю X" }]);
    expect(parseLine(line({ type: "ai-title", title: "IR monitor" }))).toMatchObject([{ kind: "title", text: "IR monitor" }]);
    expect(parseLine(line({ type: "api-request-blob", message: { content: [{ type: "text", text: "huge" }] } }))).toEqual([]);
    expect(parseLine(line({ type: "user", isSidechain: true, message: { content: "sub" } }))).toEqual([]);
    expect(parseLine(line({ type: "user", isMeta: true, message: { content: "meta" } }))).toEqual([]);
    expect(parseLine(line({ type: "user", message: { content: "<local-command-stdout>x</local-command-stdout>" } }))).toEqual([]);
    expect(parseLine(line({ type: "user", uuid: "c1", message: { content: "<command-message>ir-audit-feedback</command-message><command-name>/ir-audit-feedback</command-name><command-args>7d</command-args>" } })))
      .toMatchObject([{ kind: "user", text: "/ir-audit-feedback 7d" }]);
    expect(parseLine(line({ type: "user", uuid: "c2", message: { content: [{ type: "text", text: "<command-name>compact</command-name>" }] } }))).toMatchObject([{ kind: "user", text: "/compact" }]);
    expect(parseLine("not json")).toEqual([]);
    expect(parseLine(line({ type: "user", uuid: "c1", message: { content: "This session is being continued from a previous conversation that ran out of context. The summary…" } }))).toMatchObject([{ kind: "compact", uuid: "c1" }]);
  });
});

describe("toolSummary", () => {
  it("picks the human line per tool", () => {
    expect(toolSummary("Bash", { command: "git status\ngit log", description: "Show status" })).toBe("Show status");
    expect(toolSummary("Bash", { command: "git status\ngit log" })).toBe("git status");
    expect(toolSummary("Read", { file_path: "C:\\x\\a.rs" })).toBe("C:\\x\\a.rs");
    expect(toolSummary("Grep", { pattern: "foo", path: "src" })).toBe("foo in src");
    expect(toolSummary("Skill", { skill: "clickup-task" })).toBe("clickup-task");
    expect(toolSummary("mcp__slack__send", { channel: "#sre", text: "hi" })).toBe("#sre");
  });
});

describe("buildChat", () => {
  const blocks = [
    ...parseLine(line({ type: "ai-title", title: "T" })),
    ...parseLine(line({ type: "user", uuid: "u1", timestamp: 1, message: { content: "do it" } })),
    ...parseLine(line({ type: "assistant", uuid: "a1", timestamp: 2, message: { id: "m1", content: [{ type: "text", text: "Starting." }] } })),
    ...parseLine(line({ type: "assistant", uuid: "a2", timestamp: 3, message: { id: "m1", content: [{ type: "tool_use", id: "t1", name: "Bash", input: { command: "ls" } }] } })),
    ...parseLine(line({ type: "assistant", uuid: "a3", timestamp: 4, message: { id: "m1", content: [{ type: "tool_use", id: "t2", name: "Read", input: { file_path: "f" } }] } })),
    ...parseLine(line({ type: "user", uuid: "u2", timestamp: 5, message: { content: [{ type: "tool_result", tool_use_id: "t1", content: "a\nb" }] } })),
    ...parseLine(line({ type: "assistant", uuid: "a4", timestamp: 6, message: { id: "m2", content: [{ type: "text", text: "Done." }] } })),
    ...parseLine(line({ type: "assistant", uuid: "a5", timestamp: 7, message: { id: "m2", content: [{ type: "text", text: "Next?" }] } })),
    ...parseLine(line({ type: "system", uuid: "s1", timestamp: 8, subtype: "away_summary", content: "note" })),
  ];
  it("folds blocks into user / assistant / tools / system items", () => {
    const chat = buildChat(blocks);
    expect(chat.title).toBe("T");
    expect(chat.items.map((i) => i.kind)).toEqual(["user", "assistant", "tools", "assistant", "system"]);
    const tools = chat.items[2];
    if (tools.kind !== "tools") throw new Error("expected tools");
    expect(tools.calls.map((c) => [c.name, c.summary, c.result])).toEqual([["Bash", "ls", "a\nb"], ["Read", "f", undefined]]);
    expect(chat.pendingTools).toBe(1);
    const last = chat.items[3];
    if (last.kind !== "assistant") throw new Error("expected assistant");
    expect(last.text).toBe("Done.\n\nNext?");
  });
  it("a tool result arriving after other items still lands on its call", () => {
    const chat = buildChat([...blocks, ...parseLine(line({ type: "user", uuid: "u9", timestamp: 9, message: { content: [{ type: "tool_result", tool_use_id: "t2", content: "late", is_error: true }] } }))]);
    const tools = chat.items[2];
    if (tools.kind !== "tools") throw new Error("expected tools");
    expect(tools.calls[1]).toMatchObject({ result: "late", isError: true });
    expect(chat.pendingTools).toBe(0);
  });
});

describe("backgroundTasks", () => {
  it("lists pending Agent calls and background shells, with their descriptions", () => {
    const chat = buildChat([
      ...parseLine(line({ type: "assistant", uuid: "a1", timestamp: 1, message: { id: "m1", content: [{ type: "tool_use", id: "t1", name: "Agent", input: { description: "Разбираю JSON логов", prompt: "…", subagent_type: "general-purpose", run_in_background: true } }] } })),
      ...parseLine(line({ type: "assistant", uuid: "a2", timestamp: 2, message: { id: "m1", content: [{ type: "tool_use", id: "t2", name: "Bash", input: { command: "npm start", run_in_background: true } }] } })),
      ...parseLine(line({ type: "assistant", uuid: "a3", timestamp: 3, message: { id: "m1", content: [{ type: "tool_use", id: "t3", name: "Agent", input: { description: "done one" } }] } })),
      ...parseLine(line({ type: "assistant", uuid: "a4", timestamp: 3, message: { id: "m1", content: [{ type: "tool_use", id: "t4", name: "Bash", input: { command: "ls" } }] } })),
      ...parseLine(line({ type: "user", uuid: "u1", timestamp: 4, message: { content: [{ type: "tool_result", tool_use_id: "t3", content: "ok" }] } })),
    ]);
    expect(backgroundTasks(chat.items)).toEqual([
      { id: "t1", kind: "agent", label: "Разбираю JSON логов", since: 1, detail: "general-purpose" },
      { id: "t2", kind: "shell", label: "npm start", since: 2, detail: undefined },
    ]);
  });
});

describe("turnFooters", () => {
  it("one footer per answered turn, after its last item, with the time the agent took", () => {
    const chat = buildChat([
      ...parseLine(line({ type: "user", uuid: "u1", timestamp: 1000, message: { content: "q1" } })),
      ...parseLine(line({ type: "assistant", uuid: "a1", timestamp: 4000, message: { id: "m1", content: [{ type: "tool_use", id: "t1", name: "Bash", input: { command: "ls" } }] } })),
      ...parseLine(line({ type: "user", uuid: "r1", timestamp: 9000, message: { content: [{ type: "tool_result", tool_use_id: "t1", content: "ok" }] } })),
      ...parseLine(line({ type: "assistant", uuid: "a2", timestamp: 14000, message: { id: "m2", content: [{ type: "text", text: "done" }] } })),
      ...parseLine(line({ type: "user", uuid: "u2", timestamp: 20000, message: { content: "q2" } })),
    ]);
    const f = turnFooters(chat.items);
    expect([...f.entries()]).toEqual([[2, { start: 1000, end: 14000, ms: 13000, lastIndex: 2, open: false }]]);
    expect(fmtDuration(13000)).toBe("13s");
    expect(fmtDuration(125000)).toBe("2m 05s");
    expect(fmtDuration(4330000)).toBe("1h 12m");
  });
});

describe("ChatBuilder (incremental)", () => {
  it("keeps untouched items by identity and replaces only the ones that changed", () => {
    const b = new ChatBuilder();
    b.push(parseLine(line({ type: "user", uuid: "u1", timestamp: 1, message: { content: "q" } })));
    b.push(parseLine(line({ type: "assistant", uuid: "a1", timestamp: 2, message: { id: "m1", content: [{ type: "tool_use", id: "t1", name: "Bash", input: { command: "ls" } }] } })));
    const s1 = b.state();
    const [user1, tools1] = s1.items;
    expect(s1.pendingTools).toBe(1);
    b.push(parseLine(line({ type: "user", uuid: "r1", timestamp: 3, message: { content: [{ type: "tool_result", tool_use_id: "t1", content: "ok" }] } })));
    b.push(parseLine(line({ type: "assistant", uuid: "a2", timestamp: 4, message: { id: "m2", content: [{ type: "text", text: "done" }] } })));
    const s2 = b.state();
    expect(s2.items[0]).toBe(user1);          // untouched: same object
    expect(s2.items[1]).not.toBe(tools1);     // its call got a result: fresh object
    expect(s2.items[1].kind === "tools" && s2.items[1].calls[0].result).toBe("ok");
    expect(s2.pendingTools).toBe(0);
    expect(s2.items[2]).toMatchObject({ kind: "assistant", text: "done" });
    // a later result for a call in a replaced card still lands
    b.push(parseLine(line({ type: "assistant", uuid: "a3", timestamp: 5, message: { id: "m2", content: [{ type: "tool_use", id: "t2", name: "Read", input: { file_path: "f" } }] } })));
    b.state();
    b.push(parseLine(line({ type: "user", uuid: "r2", timestamp: 6, message: { content: [{ type: "tool_result", tool_use_id: "t2", content: "late", is_error: true }] } })));
    const s3 = b.state();
    expect(s3.items[3].kind === "tools" && s3.items[3].calls[0]).toMatchObject({ result: "late", isError: true });
    expect(buildChat([])).toEqual({ items: [], title: null, pendingTools: 0 });
  });
});
