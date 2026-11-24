import { For, Show, createSignal, onCleanup, onMount } from "solid-js";
import { dotOf, hintOf, type ServerView, type ServersView } from "../servers";

/**
 * PREFERENCES (Ctrl+,): a modal panel with sections on the left. Only
 * SERVERS for now: what is connected, what discovery found (offered, never
 * connected by itself), a field for an address by hand. Esc closes.
 */
export default function Prefs(p: {
  view?: ServersView;
  panes: (server: string) => number;
  scanning: boolean;
  error?: string | null;
  onScan: () => void;
  onConnect: (id: string) => void;
  onDisconnect: (id: string) => void;
  onRename: (id: string, name: string) => void;
  onAdd: (address: string) => void;
  onForget: (id: string) => void;
  onClose: () => void;
}) {
  let panel!: HTMLDivElement;
  let addInput!: HTMLInputElement;
  const [renaming, setRenaming] = createSignal<string | null>(null);
  const [address, setAddress] = createSignal("");
  const rows = () => p.view?.servers ?? [];
  const connected = () => rows().filter((s) => s.connected);
  const offered = () => rows().filter((s) => !s.connected);
  const online = () => connected().filter((s) => s.status.state === "online").length;

  onMount(() => {
    const prev = document.activeElement as HTMLElement | null;
    setTimeout(() => (rows().length ? panel.querySelector<HTMLElement>("button")?.focus() : addInput?.focus()), 0);
    onCleanup(() => prev?.focus?.());
  });
  const onKey = (e: KeyboardEvent) => {
    if (e.key === "Escape" && !renaming()) { e.preventDefault(); e.stopPropagation(); p.onClose(); }
  };
  const add = () => { const a = address().trim(); if (a) { p.onAdd(a); setAddress(""); } };

  const Row = (q: { s: ServerView }) => {
    const s = () => q.s;
    const hint = () => hintOf(s().status.error);
    return (
      <tr class={s().status.state === "offline" ? "bad" : ""}>
        <td>
          <button class="sw" role="switch" aria-checked={s().connected} aria-label={`connect ${s().name}`}
                  onClick={() => (s().connected ? p.onDisconnect(s().id) : p.onConnect(s().id))} />
        </td>
        <td class="who">
          <Show when={renaming() === s().id} fallback={
            <span class="nm" title="double-click to rename" onDblClick={() => setRenaming(s().id)}><span class={`dot ${dotOf(s())}`} /> {s().name}</span>}>
            <input class="ren" value={s().name} aria-label="server name" ref={(el) => setTimeout(() => { el.focus(); el.select(); }, 0)}
                   onKeyDown={(e) => {
                     if (e.key === "Enter") { p.onRename(s().id, e.currentTarget.value); setRenaming(null); }
                     else if (e.key === "Escape") { e.preventDefault(); e.stopPropagation(); setRenaming(null); }
                   }}
                   onBlur={() => setRenaming(null)} />
          </Show>
          <span class="addr" title={s().address}>{s().address}</span>
          <Show when={s().status.state === "offline" && s().status.error}>
            {(err) => <span class="err" title={err()}>{hint() ?? err()}</span>}
          </Show>
          <Show when={s().connected && !s().found && !s().status.version && s().status.state !== "online"}>
            <span class="hint">not found by the last scan</span>
          </Show>
        </td>
        <td><span class="kind">{s().kind}</span>{s().new ? <span class="kind new">new</span> : null}{s().manual ? <span class="kind">manual</span> : null}</td>
        <td class="mono" title={s().status.version ?? undefined}>{s().status.version ? `${s().status.version}${s().status.protocol != null ? ` · p${s().status.protocol}` : ""}` : "—"}</td>
        <td class="mono">{s().connected && s().status.state === "online" ? p.panes(s().id) : "—"}</td>
        <td class="act">
          <Show when={!s().connected} fallback={
            <Show when={s().status.state === "offline"} fallback={<button class="btn" onClick={() => setRenaming(s().id)}>RENAME</button>}>
              <button class="btn" onClick={p.onScan}>RETRY</button>
            </Show>}>
            <button class="btn pri" onClick={() => p.onConnect(s().id)}>CONNECT</button>
          </Show>
          <Show when={s().manual && !s().connected}><button class="btn" title="remove from the list" onClick={() => p.onForget(s().id)}>✕</button></Show>
        </td>
      </tr>
    );
  };

  return (
    <div class="palette-backdrop prefs-backdrop" onMouseDown={(e) => { if (e.target === e.currentTarget) p.onClose(); }}>
      <div ref={panel} class="prefs" role="dialog" aria-modal="true" aria-label="Preferences" onKeyDown={onKey}>
        <nav class="pnav">
          <div class="h">PREFERENCES</div>
          <a class="on" aria-current="page">SERVERS</a>
          <a class="soon">VIEW · soon</a>
          <a class="soon">NOTIFICATIONS · soon</a>
        </nav>
        <div class="pbody">
          <div class="phead">
            <h3>SERVERS</h3>
            <span class="micro">{connected().length} connected · {online()} online{offered().length ? ` · ${offered().length} found` : ""}{p.scanning ? " · scanning…" : ""}</span>
            <span class="sp" />
            <button class="btn" disabled={p.scanning} onClick={p.onScan}>↻ FIND AGAIN</button>
            <button class="btn" aria-label="close" onClick={p.onClose}>✕</button>
          </div>
          <Show when={p.error}>{(e) => <div class="err" role="alert">{e()}</div>}</Show>
          <Show when={rows().length} fallback={<div class="micro empty-srv">{p.view?.scanned ? "No servers found. Start Herdr or deck-server, or add an address below." : "Looking for servers…"}</div>}>
            <table>
              <thead><tr><th>LINK</th><th>SERVER</th><th>KIND</th><th>VERSION</th><th>PANES</th><th /></tr></thead>
              <tbody>
                <For each={connected()}>{(s) => <Row s={s} />}</For>
                <Show when={offered().length}>
                  <tr class="grp"><td colspan="6">FOUND, NOT CONNECTED</td></tr>
                  <For each={offered()}>{(s) => <Row s={s} />}</For>
                </Show>
              </tbody>
            </table>
          </Show>
          <div class="add">
            <span class="micro">+ ADD</span>
            <input ref={addInput} value={address()} onInput={(e) => setAddress(e.currentTarget.value)} onKeyDown={(e) => { if (e.key === "Enter") add(); }}
                   placeholder="user@host, or machine:path to herdr.sock (local:C:\…, user@host:/…)" aria-label="server address" spellcheck={false} />
            <button class="btn pri" disabled={!address().trim()} onClick={add}>CONNECT</button>
          </div>
          <p class="micro">Found servers are only offered: nothing connects until you switch it on. Every server is reached the same way, through a bridge on its machine; an address is machine:socket. Type user@host to add another machine (over your own SSH setup); addresses stay in Deck's local preferences.</p>
        </div>
      </div>
    </div>
  );
}
