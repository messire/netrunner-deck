import { For, Show, createEffect, createMemo, createSignal, on, onCleanup, onMount } from "solid-js";
import { api } from "../api";
import { KIND_LABEL } from "../harness";
import type { TransferState } from "./TransferDialog";

export interface HarnessPick { rel: string; kind: string; name: string }

/**
 * COPY TO / MOVE TO for the personal harness (`~/.claude`): everything or
 * the chosen items (instructions, skills, agents, commands, rules — never
 * credentials, accounts, settings or transcripts; the server refuses
 * anything else too). Items that already exist on the target: overwrite,
 * skip or keep both.
 */
export default function HarnessTransferDialog(p: {
  mode: "copy" | "move";
  items: HarnessPick[];
  /** Started from one item: only it is chosen at first. */
  only?: string;
  from: { id: string; name: string };
  to: { id: string; name: string };
  state: TransferState | null;
  onStart: (items: string[], onConflict: "overwrite" | "skip" | "keep_both") => void;
  onClose: () => void;
}) {
  const [all, setAll] = createSignal(!p.only);
  const [picked, setPicked] = createSignal<Set<string>>(new Set(p.only ? [p.only] : p.items.map((i) => i.rel)));
  const chosen = createMemo(() => (all() ? p.items.map((i) => i.rel) : p.items.map((i) => i.rel).filter((r) => picked().has(r))));
  const [existing, setExisting] = createSignal<string[]>([]);
  const [conflict, setConflict] = createSignal<"overwrite" | "skip" | "keep_both">("skip");
  let timer: number | undefined;
  createEffect(on(chosen, (items) => {
    clearTimeout(timer);
    timer = window.setTimeout(() => {
      api.machine<{ existing: string[] }>(p.to.id, "deck.transfer.conflicts", { items }).then((r) => setExisting(r.existing ?? [])).catch(() => setExisting([]));
    }, 150);
  }));
  onCleanup(() => clearTimeout(timer));
  const running = () => !!p.state && !p.state.error && !p.state.done;
  const verb = () => (p.mode === "move" ? "MOVE" : "COPY");
  const toggle = (rel: string) => setPicked((s) => { const n = new Set(s); if (n.has(rel)) n.delete(rel); else n.add(rel); return n; });
  const start = () => { if (!running() && chosen().length) p.onStart(chosen(), conflict()); };
  const onKey = (e: KeyboardEvent) => { if (e.key === "Escape") { e.preventDefault(); e.stopPropagation(); p.onClose(); } };
  onMount(() => window.addEventListener("keydown", onKey, true));
  onCleanup(() => window.removeEventListener("keydown", onKey, true));
  return (
    <div class="veil" onClick={p.onClose}>
      <div class="dlgbox wiz xfer" onClick={(e) => e.stopPropagation()}>
        <div class="dlgt">▞ {verb()} TO {p.to.name.toUpperCase()} <span class="crumb"><span>› personal harness (~/.claude)</span></span></div>
        <div class="xfer-mode">
          <label><input type="radio" name="hx-all" checked={all()} disabled={running()} onChange={() => setAll(true)} /> Transfer everything</label>
          <label><input type="radio" name="hx-all" checked={!all()} disabled={running()} onChange={() => setAll(false)} /> Transfer selected</label>
        </div>
        <div class={`xfer-list ${all() ? "dim" : ""}`} inert={all() || running()}>
          <For each={p.items} fallback={<div class="hk-empty">nothing in ~/.claude that can travel</div>}>{(i) => (
            <label class="xfer-row">
              <input type="checkbox" checked={picked().has(i.rel)} onChange={() => toggle(i.rel)} />
              <span class="kind micro">{KIND_LABEL[i.kind] ?? i.kind}</span>
              <span class="nm">{i.name}</span>
              <Show when={existing().includes(i.rel)}><span class="tag b" title={`already on ${p.to.name}`}>THERE ALREADY</span></Show>
              <span class="micro path">{i.rel}</span>
            </label>
          )}</For>
        </div>
        <div class="micro xfer-sum">{chosen().length} {chosen().length === 1 ? "item" : "items"} · settings, credentials, accounts and transcripts never travel</div>
        <Show when={existing().length}>
          <div class="xfer-mode conflict">
            <span>{existing().length} already on {p.to.name}:</span>
            <label><input type="radio" name="hx-c" checked={conflict() === "overwrite"} disabled={running()} onChange={() => setConflict("overwrite")} /> overwrite</label>
            <label><input type="radio" name="hx-c" checked={conflict() === "skip"} disabled={running()} onChange={() => setConflict("skip")} /> skip</label>
            <label><input type="radio" name="hx-c" checked={conflict() === "keep_both"} disabled={running()} onChange={() => setConflict("keep_both")} /> keep both</label>
          </div>
        </Show>
        <Show when={p.mode === "move"}><div class="xfer-warn">This removes {chosen().length === 1 ? "this item" : `these ${chosen().length} items`} from {p.from.name} after {chosen().length === 1 ? "it arrives" : "they arrive"} on {p.to.name}.</div></Show>
        <Show when={p.state}>{(st) => (
          <div class="xfer-progress">
            <div class="seg"><i style={{ width: `${st().step.percent ?? 0}%` }} /></div>
            <span class="micro">{st().error ? "✕ stopped" : st().done ? "✓ done" : st().step.phase}</span>
          </div>
        )}</Show>
        <Show when={p.state?.error}>{(e) => <div class="repo-error"><span>{e()}</span></div>}</Show>
        <Show when={p.state?.done}>{(d) => <div class="xfer-done">{d()}</div>}</Show>
        <div class="dlga">
          <span class="micro">{running() ? "closing this leaves the transfer running" : "Esc"}</span><span class="sp" />
          <button class="abtn" onClick={p.onClose}>{p.state?.done || running() ? "CLOSE" : "CANCEL"}</button>
          <Show when={!p.state?.done}>
            <button class={`abtn ${p.mode === "move" ? "danger" : "ok"}`} disabled={running() || !chosen().length} onClick={start}>{running() ? `${verb()}ING…` : verb()}</button>
          </Show>
        </div>
      </div>
    </div>
  );
}
