import { For, Show, createResource, createSignal, onCleanup, onMount } from "solid-js";
import { api } from "../api";
import { dotOf, hintOf, machineErrorOf, type Probed, type ServerView, type ServersView } from "../servers";
import AddHostDialog from "./AddHostDialog";
import Connections from "./Connections";

/**
 * PREFERENCES (Ctrl+,): a modal panel with sections on the left. SERVERS:
 * what is connected, what discovery found (offered, never connected by
 * itself), machines that cannot be reached, and + ADD (a dialog: user,
 * host, port, CHECK). CONNECTIONS: what each machine is signed in to
 * (GitHub). Esc closes.
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
  onForget: (id: string) => void;
  onRemoveHost: (machine: string) => void;
  onProbe: (user: string, host: string, port: number) => Promise<Probed[]>;
  onAddHost: (user: string, host: string, port: number, sockets: string[]) => Promise<void>;
  onClose: () => void;
}) {
  let panel!: HTMLDivElement;
  let addBtn!: HTMLButtonElement;
  const [renaming, setRenaming] = createSignal<string | null>(null);
  const [adding, setAdding] = createSignal(false);
  const [section, setSection] = createSignal<"servers" | "connections">("servers");
  /** The row whose REMOVE MACHINE waits for a second click. */
  const [armed, setArmed] = createSignal<string | null>(null);
  const rows = () => p.view?.servers ?? [];
  const down = () => p.view?.unreachable ?? [];
  const connected = () => rows().filter((s) => s.connected);
  const offered = () => rows().filter((s) => !s.connected);
  const online = () => connected().filter((s) => s.status.state === "online").length;

  onMount(() => {
    const prev = document.activeElement as HTMLElement | null;
    setTimeout(() => (rows().length ? panel.querySelector<HTMLElement>("button")?.focus() : addBtn?.focus()), 0);
    onCleanup(() => prev?.focus?.());
  });
  const onKey = (e: KeyboardEvent) => {
    if (e.key === "Escape" && armed()) { e.preventDefault(); e.stopPropagation(); setArmed(null); }
    else if (e.key === "Escape" && !renaming() && !adding()) { e.preventDefault(); e.stopPropagation(); p.onClose(); }
  };
  const closeAdd = () => { setAdding(false); setTimeout(() => addBtn?.focus(), 0); };

  /** ✕ of a machine (in row `at`): the first click arms it, the second removes the machine and all its servers. */
  const Remove = (q: { machine: string; at: string }) => (
    <Show when={armed() === q.at} fallback={
      <button class="btn" title={`remove ${q.machine} and all its servers`} aria-label={`remove machine ${q.machine}`} onClick={() => setArmed(q.at)}>✕</button>}>
      <button class="btn danger" title={`disconnect and forget every server on ${q.machine}`} onBlur={() => setArmed(null)}
              ref={(el) => setTimeout(() => el.focus(), 0)} onClick={() => { setArmed(null); p.onRemoveHost(q.machine); }}>REMOVE MACHINE?</button>
    </Show>
  );

  const Row = (q: { s: ServerView }) => {
    const s = () => q.s;
    const hint = () => hintOf(s().status.error);
    // deck-server says which build it is and whether it keeps itself up to date.
    const [info] = createResource(() => (s().connected && s().kind === "deck-server" && s().status.state === "online" ? s().id : null),
      (id) => api.machine<{ version: string; commit?: string | null; updates: boolean }>(id, "deck.server.info")
        // A deck-server too old to answer does not update itself either.
        .catch((e) => (/unknown variant|invalid request/.test(String(e)) ? { version: "", commit: null, updates: false } : null)));
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
        <td><span class="kind">{s().kind}</span>{s().new ? <span class="kind new">new</span> : null}{s().manual && !s().remote ? <span class="kind">manual</span> : null}</td>
        <td class="mono" title={info()?.commit ? `commit ${info()!.commit}` : s().status.version ?? undefined}>
          {s().status.version ? `${s().status.version}${s().status.protocol != null ? ` · p${s().status.protocol}` : ""}` : "—"}
          <Show when={info()?.commit}>{(c) => <span class="micro"> · {c().slice(0, 7)}</span>}</Show>
          <Show when={info() && !info()!.updates}><span class="kind stale" title="built by hand, without a release commit: it never updates itself — start a release build of deck-server instead">LOCAL BUILD · NO UPDATES</span></Show>
        </td>
        <td class="mono">{s().connected && s().status.state === "online" ? p.panes(s().id) : "—"}</td>
        <td class="act">
          <Show when={armed() !== s().id}>
            <Show when={!s().connected} fallback={
              <Show when={s().status.state === "offline"} fallback={<button class="btn" onClick={() => setRenaming(s().id)}>RENAME</button>}>
                <button class="btn" onClick={p.onScan}>RETRY</button>
              </Show>}>
              <button class="btn pri" onClick={() => p.onConnect(s().id)}>CONNECT</button>
            </Show>
          </Show>
          <Show when={s().machine} fallback={
            <Show when={s().manual}><button class="btn" title="remove this address from the list" aria-label={`remove ${s().name}`} onClick={() => p.onForget(s().id)}>✕</button></Show>}>
            {(m) => <Remove machine={m()} at={s().id} />}
          </Show>
        </td>
      </tr>
    );
  };

  return (
    <div class="palette-backdrop prefs-backdrop" onMouseDown={(e) => { if (e.target === e.currentTarget) p.onClose(); }}>
      <div ref={panel} class="prefs" role="dialog" aria-modal="true" aria-label="Preferences" onKeyDown={onKey}>
        <nav class="pnav">
          <div class="h">PREFERENCES</div>
          <a class={section() === "servers" ? "on" : ""} aria-current={section() === "servers" ? "page" : undefined} onClick={() => setSection("servers")}>SERVERS</a>
          <a class={section() === "connections" ? "on" : ""} aria-current={section() === "connections" ? "page" : undefined} onClick={() => setSection("connections")}>CONNECTIONS</a>
          <a class="soon">VIEW · soon</a>
          <a class="soon">NOTIFICATIONS · soon</a>
        </nav>
        <Show when={section() === "connections"}>
          <div class="pbody">
            <div class="phead">
              <h3>CONNECTIONS</h3>
              <span class="micro">per machine: what its deck-server is signed in to</span>
              <span class="sp" />
              <button class="btn" aria-label="close" onClick={p.onClose}>✕</button>
            </div>
            <Connections servers={connected().filter((s) => s.kind === "deck-server" && s.status.state === "online").map((s) => ({ id: s.id, name: s.name }))} />
          </div>
        </Show>
        <div class="pbody" hidden={section() !== "servers"}>
          <div class="phead">
            <h3>SERVERS</h3>
            <span class="micro">{connected().length} connected · {online()} online{offered().length ? ` · ${offered().length} found` : ""}{p.scanning ? " · scanning…" : ""}</span>
            <span class="sp" />
            <button class="btn" disabled={p.scanning} onClick={p.onScan}>↻ FIND AGAIN</button>
            <button class="btn" aria-label="close" onClick={p.onClose}>✕</button>
          </div>
          <Show when={p.error}>{(e) => <div class="err" role="alert">{e()}</div>}</Show>
          <Show when={rows().length || down().length} fallback={<div class="micro empty-srv">{p.view?.scanned ? "No servers found. Start Herdr or deck-server here, or + ADD a machine." : "Looking for servers…"}</div>}>
            <table>
              <thead><tr><th>LINK</th><th>SERVER</th><th>KIND</th><th>VERSION</th><th>PANES</th><th /></tr></thead>
              <tbody>
                <For each={connected()}>{(s) => <Row s={s} />}</For>
                <Show when={offered().length}>
                  <tr class="grp"><td colspan="6">FOUND, NOT CONNECTED</td></tr>
                  <For each={offered()}>{(s) => <Row s={s} />}</For>
                </Show>
                <Show when={down().length}>
                  <tr class="grp"><td colspan="6">MACHINES NOT REACHABLE</td></tr>
                  <For each={down()}>{(m) => (
                    <tr class="gone">
                      <td><span class="sw off" aria-hidden="true" /></td>
                      <td class="who">
                        <span class="nm"><span class="dot off" /> {m.machine}</span>
                        <span class="err" title={m.error}>unreachable: {machineErrorOf(m.error)}</span>
                      </td>
                      <td><span class="kind">machine</span></td>
                      <td class="mono">—</td>
                      <td class="mono">—</td>
                      <td class="act"><Show when={armed() !== `machine:${m.machine}`}><button class="btn" onClick={p.onScan} disabled={p.scanning}>RETRY</button></Show><Remove machine={m.machine} at={`machine:${m.machine}`} /></td>
                    </tr>
                  )}</For>
                </Show>
              </tbody>
            </table>
          </Show>
          <div class="add">
            <button ref={addBtn} class="btn pri" onClick={() => setAdding(true)}>+ ADD</button>
            <span class="micro">a machine by user, address and port</span>
          </div>
        </div>
      </div>
      <Show when={adding()}>
        <AddHostDialog onProbe={p.onProbe} onAdd={p.onAddHost} onClose={closeAdd} />
      </Show>
    </div>
  );
}
