import { For, createEffect, createMemo, createSignal, onCleanup, onMount } from "solid-js";

/** One palette entry; `keys` are extra words the filter matches (ids, agent names). */
export interface PaletteItem {
  id: string;
  label: string;
  hint?: string;
  keys?: string;
  danger?: boolean;
  run: () => void;
}

/**
 * Command palette (Ctrl+K) and account picker (F1): a filter box over a list.
 * Type to filter, ↑/↓ to move, Enter to run, Esc to close; a digit alone picks
 * that row when `digits` is on (accounts are numbered like `cswap` shows them).
 */
export default function Palette(p: { title: string; items: PaletteItem[]; digits?: boolean; onClose: () => void }) {
  let input!: HTMLInputElement;
  const [q, setQ] = createSignal("");
  const [cur, setCur] = createSignal(0);

  const shown = createMemo(() => {
    const words = q().toLowerCase().split(/\s+/).filter(Boolean);
    if (!words.length) return p.items;
    return p.items.filter((it) => {
      const hay = `${it.label} ${it.hint ?? ""} ${it.keys ?? ""}`.toLowerCase();
      return words.every((w) => hay.includes(w));
    });
  });
  createEffect(() => { shown(); setCur(0); });

  const run = (it: PaletteItem | undefined) => { if (!it) return; p.onClose(); it.run(); };

  const onKey = (e: KeyboardEvent) => {
    if (e.key === "Escape") { e.preventDefault(); p.onClose(); return; }
    if (e.key === "ArrowDown") { e.preventDefault(); setCur((c) => Math.min(shown().length - 1, c + 1)); return; }
    if (e.key === "ArrowUp") { e.preventDefault(); setCur((c) => Math.max(0, c - 1)); return; }
    if (e.key === "Enter") { e.preventDefault(); run(shown()[cur()]); return; }
    if (p.digits && q() === "" && /^[1-9]$/.test(e.key)) {
      const it = p.items[Number(e.key) - 1];
      if (it) { e.preventDefault(); run(it); }
    }
  };

  onMount(() => {
    // Focus after the keydown that opened us has fully dispatched; focusing
    // synchronously inside that handler loses to xterm's textarea.
    setTimeout(() => input.focus(), 0);
    const onDown = (e: MouseEvent) => { if (!(e.target as HTMLElement).closest(".palette")) p.onClose(); };
    window.addEventListener("mousedown", onDown, true);
    onCleanup(() => window.removeEventListener("mousedown", onDown, true));
  });

  return (
    <div class="palette-backdrop">
      <div class="palette" onKeyDown={onKey}>
        <div class="palette-head"><span class="plate" style="margin:0">{p.title}</span><span class="micro">↑↓ MOVE · ENTER RUN · ESC CLOSE{p.digits ? " · 1-9 PICK" : ""}</span></div>
        <input ref={input} class="palette-input" value={q()} onInput={(e) => setQ(e.currentTarget.value)} placeholder="type to filter…" spellcheck={false} />
        <div class="palette-list">
          <For each={shown()} fallback={<div class="menu-item off">nothing matches</div>}>
            {(it, i) => (
              <div class={`menu-item ${it.danger ? "danger" : ""} ${i() === cur() ? "cur" : ""}`} onMouseEnter={() => setCur(i())} onClick={() => run(it)}>
                <span>{p.digits ? <b class="num">{String(i() + 1).padStart(2, "0")}</b> : null}{it.label}</span>
                {it.hint ? <span class="hint">{it.hint}</span> : null}
              </div>
            )}
          </For>
        </div>
      </div>
    </div>
  );
}
