import { For, Show, createSignal, onCleanup, onMount } from "solid-js";

/**
 * One menu item. `danger` items are destructive (close): the first click arms
 * them ("CONFIRM?"), the second within 3 s runs them, so a stray right-click
 * never kills a session.
 */
export interface MenuItem {
  label: string;
  hint?: string;
  danger?: boolean;
  disabled?: boolean;
  run: () => void;
}

export interface MenuState { x: number; y: number; title: string; items: MenuItem[] }

/** Context menu anchored at a point; closes on Esc, outside click, or after an action. */
export default function ContextMenu(p: { menu: MenuState; onClose: () => void }) {
  let el!: HTMLDivElement;
  const [armed, setArmed] = createSignal<number | null>(null);
  let disarm: number | undefined;

  const click = (item: MenuItem, i: number) => {
    if (item.disabled) return;
    if (item.danger && armed() !== i) {
      setArmed(i);
      clearTimeout(disarm);
      disarm = window.setTimeout(() => setArmed(null), 3000);
      return;
    }
    p.onClose();
    item.run();
  };

  onMount(() => {
    // Keep the menu inside the window.
    const r = el.getBoundingClientRect();
    const x = Math.min(p.menu.x, window.innerWidth - r.width - 8);
    const y = Math.min(p.menu.y, window.innerHeight - r.height - 8);
    el.style.left = `${Math.max(8, x)}px`;
    el.style.top = `${Math.max(8, y)}px`;
    const onKey = (e: KeyboardEvent) => { if (e.key === "Escape") { e.stopPropagation(); p.onClose(); } };
    const onDown = (e: MouseEvent) => { if (!el.contains(e.target as Node)) p.onClose(); };
    window.addEventListener("keydown", onKey, true);
    window.addEventListener("mousedown", onDown, true);
    onCleanup(() => { window.removeEventListener("keydown", onKey, true); window.removeEventListener("mousedown", onDown, true); clearTimeout(disarm); });
  });

  return (
    <div class="menu" ref={el} onContextMenu={(e) => e.preventDefault()}>
      <div class="menu-title">{p.menu.title}</div>
      <For each={p.menu.items}>
        {(item, i) => (
          <div class={`menu-item ${item.danger ? "danger" : ""} ${item.disabled ? "off" : ""} ${armed() === i() ? "armed" : ""}`} onClick={() => click(item, i())}>
            <span>{armed() === i() ? "CONFIRM?" : item.label}</span>
            <Show when={item.hint}><span class="hint">{item.hint}</span></Show>
          </div>
        )}
      </For>
    </div>
  );
}
