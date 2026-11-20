import { For, Show, createMemo, createSignal, onCleanup, onMount } from "solid-js";
import { filterItems } from "../quickhacks";

export interface KnownRepo { name: string; root: string; open: boolean }

/** "+" next to SESSIONS: open a repository as a new space — a known one from the list, or a folder path. */
export default function NewRepoBox(p: { known: KnownRepo[]; onPick: (root: string) => void; onClose: () => void }) {
  const [q, setQ] = createSignal("");
  const [at, setAt] = createSignal(0);
  let input!: HTMLInputElement;
  const rows = createMemo(() => filterItems(p.known.filter((r) => !r.open).map((r) => ({ ...r, description: r.root })), q()));
  const looksLikePath = () => /^([a-zA-Z]:[\\/]|\/|\\\\|~)/.test(q().trim());
  const go = () => {
    if (looksLikePath()) { const v = q().trim(); p.onPick(v); p.onClose(); return; }
    const r = rows()[at()];
    if (r) { p.onPick(r.root); p.onClose(); }
  };
  const onKey = (e: KeyboardEvent) => {
    if (e.key === "Escape") { e.preventDefault(); p.onClose(); }
    else if (e.key === "Enter") { e.preventDefault(); go(); }
    else if (e.key === "ArrowDown") { e.preventDefault(); setAt((i) => Math.min(rows().length - 1, i + 1)); }
    else if (e.key === "ArrowUp") { e.preventDefault(); setAt((i) => Math.max(0, i - 1)); }
    e.stopPropagation();
  };
  onMount(() => { input.focus(); window.addEventListener("keydown", onKey, true); });
  onCleanup(() => window.removeEventListener("keydown", onKey, true));
  return (
    <div class="veil" onClick={p.onClose}>
      <div class="dlgbox wiz" onClick={(e) => e.stopPropagation()}>
        <div class="dlgt">▞ OPEN REPOSITORY <span class="crumb"><span>› a known repo, or a folder path</span></span></div>
        <input ref={input} class="arc-q wide" placeholder="search known repos… or paste a path (C:\\Workspace\\…)" value={q()} onInput={(e) => { setQ(e.currentTarget.value); setAt(0); }} />
        <div class="wiz-list">
          <Show when={looksLikePath()} fallback={
            <Show when={rows().length} fallback={<div class="hk-empty">▞ {p.known.length ? "EVERYTHING KNOWN IS OPEN — paste a path" : "NO KNOWN REPOS — paste a path"}</div>}>
              <For each={rows()}>{(r, i) => (
                <button class={`cm-opt ${i() === at() ? "cur" : ""}`} onMouseMove={() => setAt(i())} onClick={() => { p.onPick(r.root); p.onClose(); }}>
                  <span class="n">{i() < 9 ? i() + 1 : ""}</span><span class="lb">{r.name}<span class="hint">{r.root}</span></span>
                </button>
              )}</For>
            </Show>
          }>
            <button class="cm-opt cur" onClick={go}><span class="n">↵</span><span class="lb">open folder<span class="hint">{q().trim()}</span></span></button>
          </Show>
        </div>
        <div class="dlga"><span class="micro">↑↓ · Enter · Esc</span><span class="sp" /><button class="abtn" onClick={p.onClose}>CANCEL</button><button class="abtn ok" onClick={go}>OPEN</button></div>
      </div>
    </div>
  );
}
