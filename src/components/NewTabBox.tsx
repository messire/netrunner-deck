import { For, createSignal, onCleanup, onMount } from "solid-js";

/** What a new tab runs. `command` null = a plain shell. */
export interface TabKind { id: string; label: string; hint: string; command: string | null }

export const TAB_KINDS: TabKind[] = [
  { id: "claude", label: "CLAUDE", hint: "claude — a new session", command: "claude" },
];

/** Picker shown on "+ new tab": digits or ↑↓ + Enter choose, a typed command runs instead. */
export default function NewTabBox(p: { initial: string; onPick: (kind: TabKind) => void; onClose: () => void }) {
  const start = Math.max(0, TAB_KINDS.findIndex((k) => k.id === p.initial));
  const [at, setAt] = createSignal(start);
  const [custom, setCustom] = createSignal("");
  let input!: HTMLInputElement;
  // Run the pick before closing: the owner's <Show> accessor behind onPick is gone once it closes.
  const pick = (k: TabKind) => { p.onPick(k); p.onClose(); };
  const go = () => {
    const c = custom().trim();
    if (c) pick({ id: "custom", label: c.toUpperCase(), hint: c, command: c });
    else pick(TAB_KINDS[at()]);
  };
  const onKey = (e: KeyboardEvent) => {
    if (e.key === "Escape") { e.preventDefault(); p.onClose(); }
    else if (e.key === "Enter") { e.preventDefault(); go(); }
    else if (e.key === "ArrowDown") { e.preventDefault(); setAt((i) => (i + 1) % TAB_KINDS.length); }
    else if (e.key === "ArrowUp") { e.preventDefault(); setAt((i) => (i - 1 + TAB_KINDS.length) % TAB_KINDS.length); }
    else if (/^[1-9]$/.test(e.key) && !custom() && Number(e.key) <= TAB_KINDS.length) { e.preventDefault(); pick(TAB_KINDS[Number(e.key) - 1]); }
    e.stopPropagation();
  };
  onMount(() => { input.focus(); window.addEventListener("keydown", onKey, true); });
  onCleanup(() => window.removeEventListener("keydown", onKey, true));
  return (
    <div class="veil" onClick={p.onClose}>
      <div class="dlgbox newtab" onClick={(e) => e.stopPropagation()}>
        <div class="dlgt">▞ NEW TAB</div>
        <For each={TAB_KINDS}>{(k, i) => (
          <button class={`cm-opt ${i() === at() ? "cur" : ""}`} onMouseMove={() => setAt(i())} onClick={() => pick(k)}>
            <span class="n">{i() + 1}</span><span class="lb">{k.label}<span class="hint">{k.hint}</span></span>
          </button>
        )}</For>
        <input ref={input} class="arc-q wide" placeholder="…or a command to run instead, e.g. claude --resume (Enter)" value={custom()} onInput={(e) => setCustom(e.currentTarget.value)} />
        <div class="dlga"><span class="micro">digits · ↑↓ · Enter · Esc</span><span class="sp" /><button class="abtn" onClick={p.onClose}>CANCEL</button><button class="abtn ok" onClick={go}>OPEN</button></div>
      </div>
    </div>
  );
}
