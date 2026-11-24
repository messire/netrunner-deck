import { For, Show, createSignal } from "solid-js";
import { allow, answerQuestions, askSummary, deny, questionsOf, type Ask } from "../agent";

/**
 * A question a headless Claude session waits on, as a card in the chat:
 * a tool permission (allow / always allow / deny with a note),
 * AskUserQuestion (options, several when the question allows, or own text),
 * or the plan of plan mode (approve, approve and accept edits, keep planning).
 * `answer` gets the response the SDK's `canUseTool` would return.
 */
export default function AskCard(p: {
  ask: Ask;
  md: (text: string) => string;
  answer: (response: Record<string, unknown>, then?: "acceptEdits") => Promise<void>;
}) {
  const [busy, setBusy] = createSignal(false);
  const [note, setNote] = createSignal("");
  const send = async (response: Record<string, unknown>, then?: "acceptEdits") => {
    if (busy()) return;
    setBusy(true);
    try { await p.answer(response, then); } finally { setBusy(false); }
  };
  const ask = () => p.ask;

  if (ask().tool === "AskUserQuestion") {
    const qs = questionsOf(ask());
    const [picked, setPicked] = createSignal<Record<string, string[]>>({});
    const [own, setOwn] = createSignal<Record<string, string>>({});
    const toggle = (q: string, label: string, multi: boolean) => setPicked((m) => {
      const cur = m[q] ?? [];
      const next = multi ? (cur.includes(label) ? cur.filter((x) => x !== label) : [...cur, label]) : [label];
      return { ...m, [q]: next };
    });
    const chosen = () => {
      const out: Record<string, string[]> = {};
      for (const q of qs) {
        const typed = (own()[q.question] ?? "").trim();
        out[q.question] = [...(picked()[q.question] ?? []), ...(typed ? [typed] : [])];
      }
      return out;
    };
    const complete = () => qs.every((q) => chosen()[q.question].length > 0);
    return (
      <div class="chat-menu ask">
        <div class="cm-title">▲ CLAUDE ASKS</div>
        <For each={qs}>{(q) => (
          <div class="ask-q">
            <div class="ask-qt"><Show when={q.header}><span class="ask-tag">{q.header}</span></Show>{q.question}</div>
            <For each={q.options}>{(o, i) => (
              <button class={`cm-opt ${(picked()[q.question] ?? []).includes(o.label) ? "cur" : ""}`} disabled={busy()} onClick={() => toggle(q.question, o.label, q.multiSelect)}>
                <span class="n">{q.multiSelect ? ((picked()[q.question] ?? []).includes(o.label) ? "■" : "□") : i() + 1}</span>
                <span class="lb">{o.label}<Show when={o.description}><span class="hint">{o.description}</span></Show></span>
              </button>
            )}</For>
            <input class="ask-own" placeholder="…or your own answer" value={own()[q.question] ?? ""} disabled={busy()}
              onInput={(e) => { const v = e.currentTarget.value; setOwn((m) => ({ ...m, [q.question]: v })); }}
              onKeyDown={(e) => { if (e.key === "Enter" && complete()) void send(answerQuestions(ask(), chosen())); }} />
          </div>
        )}</For>
        <div class="cm-foot">
          <button class="abtn hot" disabled={busy() || !complete()} onClick={() => send(answerQuestions(ask(), chosen()))}>ANSWER</button>
          <button class="abtn" disabled={busy()} onClick={() => send(deny("The user did not answer the question."))}>SKIP</button>
          <span class="sp" />
        </div>
      </div>
    );
  }

  const sum = askSummary(ask());
  if (ask().tool === "ExitPlanMode") {
    return (
      <div class="chat-menu ask plan">
        <div class="cm-title">▲ THE PLAN IS READY</div>
        <Show when={sum.body}><div class="mdtext ask-plan" innerHTML={p.md(sum.body)} /></Show>
        <input class="ask-own" placeholder="what to change (for KEEP PLANNING)" value={note()} disabled={busy()} onInput={(e) => setNote(e.currentTarget.value)} />
        <div class="cm-foot">
          <button class="abtn hot" disabled={busy()} onClick={() => send(allow(ask()), "acceptEdits")}>APPROVE · AUTO-ACCEPT EDITS</button>
          <button class="abtn" disabled={busy()} onClick={() => send(allow(ask()))}>APPROVE</button>
          <button class="abtn" disabled={busy()} onClick={() => send(deny(note().trim() || "Keep planning."))}>KEEP PLANNING</button>
          <span class="sp" />
        </div>
      </div>
    );
  }

  return (
    <div class="chat-menu ask">
      <div class="cm-title">▲ ALLOW {ask().tool.toUpperCase()}?</div>
      <div class="ask-head">{sum.head}</div>
      <Show when={sum.body}><pre class="tool-io ask-body">{sum.body}</pre></Show>
      <input class="ask-own" placeholder="…or tell Claude what to do instead (DENY)" value={note()} disabled={busy()}
        onInput={(e) => setNote(e.currentTarget.value)}
        onKeyDown={(e) => { if (e.key === "Enter" && note().trim()) void send(deny(note().trim())); }} />
      <div class="cm-foot">
        <button class="abtn hot" disabled={busy()} onClick={() => send(allow(ask()))}>ALLOW</button>
        <Show when={ask().suggestions.length}>
          <button class="abtn" disabled={busy()} title="allow, and don't ask again for this in this session" onClick={() => send(allow(ask(), ask().input, true))}>ALWAYS ALLOW</button>
        </Show>
        <button class="abtn" disabled={busy()} onClick={() => send(deny(note().trim() || undefined))}>DENY</button>
        <span class="sp" />
      </div>
    </div>
  );
}
