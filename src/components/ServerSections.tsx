import { For, Show, createSignal, type JSX } from "solid-js";
import { kindLabel, type Source } from "../sources";

/**
 * HARNESS, UPLINK, ARCHIVE and QUICKHACKS show the server picked on the
 * deck tab. With ALL and more than one server, each server is a section
 * that folds like the server sections of SESSIONS (remembered per view).
 */
const foldKey = (view: string) => `deck.${view}.folded`;
function loadFolded(view: string): Record<string, boolean> {
  try { return JSON.parse(localStorage.getItem(foldKey(view)) ?? "{}"); } catch { return {}; }
}

export default function ServerSections<T>(p: {
  view: string;
  sources: Source<T>[];
  /** The number on a section's header (files, sessions, pulls, hacks). */
  count: (s: Source<T>) => number | string;
  children: (s: Source<T>) => JSX.Element;
}) {
  const [folded, setFolded] = createSignal<Record<string, boolean>>(loadFolded(p.view));
  const fold = (id: string) => {
    const next = { ...folded(), [id]: !folded()[id] };
    setFolded(next);
    try { localStorage.setItem(foldKey(p.view), JSON.stringify(next)); } catch { /* per-viewer convenience only */ }
  };
  return (
    <Show when={p.sources.length > 1} fallback={<Show when={p.sources[0]}>{(s) => (
      // One server: its failure stands in for its data (never an endless SCANNING…).
      <Show when={!(s().error && s().data === undefined)} fallback={<div class="arc-empty src-err" title={s().error ?? ""}>✕ {s().error}</div>}>{p.children(s())}</Show>
    )}</Show>}>
      <For each={p.sources}>{(s) => (
        <div class={`srv-sec view-sec ${s.error ? "down" : ""}`}>
          <div class="row srv" title={`${s.error ?? s.name} · click to ${folded()[s.id] ? "unfold" : "fold"}`} onClick={() => fold(s.id)}>
            <button class="car cy" aria-expanded={!folded()[s.id]} title={folded()[s.id] ? "expand" : "collapse"} onClick={(e) => { e.stopPropagation(); fold(s.id); }}>{folded()[s.id] ? "▸" : "▾"}</button>
            <span class={`dot ${s.error ? "bad" : s.data === undefined ? "wait" : "ok"}`} aria-hidden="true" />
            <span class="nm">{s.name.toUpperCase()}</span>
            <span class="tag srv-kind">{kindLabel(s.kind)}</span>
            <Show when={s.error}><span class="tag b" title={s.error ?? ""}>UNAVAILABLE</span></Show>
            <span class="cnt">{p.count(s)}</span>
          </div>
          <div class="srv-body" hidden={!!folded()[s.id]}>
            <Show when={s.error} fallback={p.children(s)}>{(m) => <div class="arc-empty src-err" title={m()}>✕ {m()}</div>}</Show>
          </div>
        </div>
      )}</For>
    </Show>
  );
}

/** Which server a single-server view shows: `ON NAME · DECK`. */
export function SourceTag(p: { sources: Source<unknown>[] }) {
  return (
    <Show when={p.sources.length === 1 && p.sources[0]}>{(s) => (
      <span class="src-on micro" title={s().id}>ON <b>{s().name.toUpperCase()}</b> · {kindLabel(s().kind)}</span>
    )}</Show>
  );
}
