import { For, Show, createSignal, onCleanup, onMount } from "solid-js";
import { dotOf, type ServerView } from "../servers";
export { dotOf } from "../servers";

/** What the deck tab shows: every connected server, or one. */
export type Scope = "all" | string;

const versionOf = (s: ServerView) =>
  s.status.version ? `${s.kind} ${s.status.version}${s.status.protocol != null ? ` · p${s.status.protocol}` : ""}` : s.kind;

/**
 * The server menu under the active NETRUNNER DECK tab: show ALL or one
 * server (a filter, nothing reconnects), connect a found one, open SERVERS.
 * ↑↓ move, Enter picks, Esc closes; Alt+0…9 work without opening it.
 */
export default function ServerMenu(p: {
  connected: ServerView[];
  offered: ServerView[];
  scope: Scope;
  panes: (server: string) => number;
  onScope: (s: Scope) => void;
  onConnect: (id: string) => void;
  onManage: () => void;
  onClose: () => void;
}) {
  let box!: HTMLDivElement;
  const items = () => [...box.querySelectorAll<HTMLElement>("[role^=menuitem]:not([aria-disabled=true])")];
  const move = (d: number) => {
    const xs = items();
    const i = xs.indexOf(document.activeElement as HTMLElement);
    xs[(i + d + xs.length) % xs.length]?.focus();
  };
  const onKey = (e: KeyboardEvent) => {
    if (e.key === "Escape") { e.preventDefault(); e.stopPropagation(); p.onClose(); }
    else if (e.key === "ArrowDown") { e.preventDefault(); move(1); }
    else if (e.key === "ArrowUp") { e.preventDefault(); move(-1); }
  };
  const [ready, setReady] = createSignal(false);
  onMount(() => {
    setTimeout(() => { (box.querySelector<HTMLElement>("[aria-checked=true]") ?? items()[0])?.focus(); setReady(true); }, 0);
    const onDown = (e: MouseEvent) => { const t = e.target as HTMLElement; if (!t.closest(".srv-menu") && !t.closest(".vtab.deck")) p.onClose(); };
    window.addEventListener("mousedown", onDown, true);
    onCleanup(() => window.removeEventListener("mousedown", onDown, true));
  });
  const pick = (fn: () => void) => { p.onClose(); fn(); };
  const total = () => p.connected.reduce((n, s) => n + p.panes(s.id), 0);
  const online = () => p.connected.filter((s) => s.status.state === "online").length;

  return (
    <div ref={box} class="srv-menu" id="srv-menu" role="menu" aria-label="Servers" onKeyDown={onKey} data-ready={ready()}>
      <div class="ttl">SHOW SESSIONS OF</div>
      <button role="menuitemradio" aria-checked={p.scope === "all"} onClick={() => pick(() => p.onScope("all"))}>
        <span class="chk">{p.scope === "all" ? "✓" : ""}</span>
        <span class={`dot ${online() === p.connected.length ? "ok" : online() ? "warn" : "bad"}`} />
        <span class="txt"><span class="nm">ALL SERVERS</span><span class="sub">{online()} of {p.connected.length} online · {total()} panes</span></span>
        <span class="cnt">Alt+0</span>
      </button>
      <Show when={p.connected.length}><hr /></Show>
      <For each={p.connected}>{(s, i) => (
        <button role="menuitemradio" aria-checked={p.scope === s.id} onClick={() => pick(() => p.onScope(s.id))} title={s.status.error ?? s.address}>
          <span class="chk">{p.scope === s.id ? "✓" : ""}</span>
          <span class={`dot ${dotOf(s)}`} />
          <span class="txt"><span class="nm">{s.name}</span><span class="sub">{s.status.state === "offline" ? "OFFLINE" : versionOf(s)}</span></span>
          <span class="cnt">{p.panes(s.id)}{i() < 9 ? ` · Alt+${i() + 1}` : ""}</span>
        </button>
      )}</For>
      <Show when={p.offered.length}>
        <hr />
        <div class="ttl">FOUND, NOT CONNECTED</div>
        <For each={p.offered}>{(s) => (
          <button role="menuitem" class="offer" onClick={() => pick(() => p.onConnect(s.id))} title={s.status.error ?? s.address}>
            <span class="chk" />
            <span class={`dot off ${s.new ? "new" : ""}`} />
            <span class="txt"><span class="nm">{s.name}</span><span class="sub">{s.status.state === "offline" ? "no access" : versionOf(s)}</span></span>
            <span class="cnt cy">+ CONNECT</span>
          </button>
        )}</For>
      </Show>
      <hr />
      <button role="menuitem" class="foot" onClick={() => pick(p.onManage)}><span>⚙ MANAGE SERVERS…</span><span class="cnt">Ctrl+,</span></button>
    </div>
  );
}
