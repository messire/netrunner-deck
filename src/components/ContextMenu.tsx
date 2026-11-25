import { For, Show, createSignal, onCleanup, onMount } from "solid-js";

/**
 * One menu item. `danger` items are destructive (close): the first click arms
 * them ("CONFIRM?"), the second within 3 s runs them, so a stray right-click
 * never kills a session. An item with `items` opens them as a submenu.
 */
export interface MenuItem {
  label: string;
  hint?: string;
  danger?: boolean;
  disabled?: boolean;
  run?: () => void;
  items?: MenuItem[];
}

export interface MenuState { x: number; y: number; title: string; items: MenuItem[] }

/** Context menu anchored at a point; closes on Esc, outside click, or after an action. */
export default function ContextMenu(p: { menu: MenuState; onClose: () => void }) {
  let el!: HTMLDivElement;
  let subEl: HTMLDivElement | undefined;
  const [armed, setArmed] = createSignal<number | null>(null);
  const [sub, setSub] = createSignal<{ at: number; items: MenuItem[]; x: number; y: number } | null>(null);
  let disarm: number | undefined;

  const click = (item: MenuItem, i: number) => {
    if (item.disabled || item.items) return;
    if (item.danger && armed() !== i) {
      setArmed(i);
      clearTimeout(disarm);
      disarm = window.setTimeout(() => setArmed(null), 3000);
      return;
    }
    p.onClose();
    item.run?.();
  };
  const openSub = (item: MenuItem, i: number, row: HTMLElement) => {
    if (!item.items || item.disabled) { setSub(null); return; }
    if (sub()?.at === i) return;
    const r = row.getBoundingClientRect();
    setSub({ at: i, items: item.items, x: r.right + 2, y: r.top - 5 });
  };

  const place = (node: HTMLElement, x: number, y: number) => {
    const r = node.getBoundingClientRect();
    node.style.left = `${Math.max(8, Math.min(x, window.innerWidth - r.width - 8))}px`;
    node.style.top = `${Math.max(8, Math.min(y, window.innerHeight - r.height - 8))}px`;
  };

  onMount(() => {
    // Keep the menu inside the window.
    place(el, p.menu.x, p.menu.y);
    const onKey = (e: KeyboardEvent) => { if (e.key === "Escape") { e.stopPropagation(); if (sub()) setSub(null); else p.onClose(); } };
    const onDown = (e: MouseEvent) => { const t = e.target as Node; if (!el.contains(t) && !subEl?.contains(t)) p.onClose(); };
    window.addEventListener("keydown", onKey, true);
    window.addEventListener("mousedown", onDown, true);
    onCleanup(() => { window.removeEventListener("keydown", onKey, true); window.removeEventListener("mousedown", onDown, true); clearTimeout(disarm); });
  });

  const Row = (q: { item: MenuItem; i: number; inSub: boolean }) => (
    <div class={`menu-item ${q.item.danger ? "danger" : ""} ${q.item.disabled ? "off" : ""} ${!q.inSub && armed() === q.i ? "armed" : ""} ${!q.inSub && sub()?.at === q.i ? "cur" : ""}`}
      onMouseEnter={(e) => { if (!q.inSub) openSub(q.item, q.i, e.currentTarget); }}
      onClick={(e) => { if (q.inSub) { if (!q.item.disabled) { p.onClose(); q.item.run?.(); } } else if (q.item.items) openSub(q.item, q.i, e.currentTarget); else click(q.item, q.i); }}>
      <span>{!q.inSub && armed() === q.i ? "CONFIRM?" : q.item.label}</span>
      <Show when={q.item.hint || q.item.items}><span class="hint">{q.item.hint ?? ""}{q.item.items ? " ▸" : ""}</span></Show>
    </div>
  );

  return (
    <>
      <div class="menu" ref={el} onContextMenu={(e) => e.preventDefault()}>
        <div class="menu-title">{p.menu.title}</div>
        <For each={p.menu.items}>{(item, i) => <Row item={item} i={i()} inSub={false} />}</For>
      </div>
      <Show when={sub()}>{(s) => (
        <div class="menu sub" ref={(n) => { subEl = n; queueMicrotask(() => place(n, s().x, s().y)); }} onContextMenu={(e) => e.preventDefault()}>
          <For each={s().items} fallback={<div class="menu-item off"><span>no other server connected</span></div>}>{(item, i) => <Row item={item} i={i()} inSub />}</For>
        </div>
      )}</Show>
    </>
  );
}
