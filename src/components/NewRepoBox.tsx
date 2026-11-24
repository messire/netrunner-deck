import { For, Show, createMemo, createSignal, onCleanup, onMount } from "solid-js";
import { filterItems } from "../quickhacks";

export interface KnownRepo { name: string; root: string; open: boolean }

/** "+" next to SESSIONS: open a repository as a new space — a known one from the list, or a folder path. */
export default function NewRepoBox(p: {
  known: KnownRepo[];
  /** Opens the folder; a rejection is shown in the box, which stays open. */
  onPick: (root: string) => Promise<void>;
  onClose: () => void;
  /** What a typed path is on the server's machine (null: the server cannot tell). */
  check?: (path: string) => Promise<{ path: string; exists: boolean; dir: boolean } | null>;
  /** Creates the folder on the server's machine. */
  create?: (path: string) => Promise<unknown>;
  /** The server the space opens on. */
  where?: string;
  /** That server is on another machine: the list and the path are that machine's. */
  remote?: boolean;
  loading?: boolean;
}) {
  const [q, setQ] = createSignal("");
  const [at, setAt] = createSignal(0);
  let input!: HTMLInputElement;
  const rows = createMemo(() => filterItems(p.known.filter((r) => !r.open).map((r) => ({ ...r, description: r.root })), q()));
  const looksLikePath = () => /^([a-zA-Z]:[\\/]|\/|\\\\|~)/.test(q().trim());
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  /** A typed folder that is not there: offered to be created. */
  const [missing, setMissing] = createSignal<string | null>(null);
  const run = async (work: () => Promise<void>) => {
    if (busy()) return;
    setBusy(true); setError(null);
    try { await work(); } catch (e) { setError(String(e).replace(/^Error: /, "")); } finally { setBusy(false); }
  };
  const open = (root: string) => run(async () => { await p.onPick(root); p.onClose(); });
  const go = () => {
    if (looksLikePath()) {
      const v = q().trim();
      return run(async () => {
        const state = await p.check?.(v).catch((e) => { throw new Error(`cannot check ${v}: ${e}`); });
        if (state && !state.exists) { setMissing(state.path); return; }
        if (state && !state.dir) throw new Error(`${state.path} is a file, not a folder`);
        await p.onPick(state?.path ?? v);
        p.onClose();
      });
    }
    const r = rows()[at()];
    if (r) void open(r.root);
  };
  const createAndOpen = () => run(async () => {
    const path = missing();
    if (!path || !p.create) return;
    await p.create(path);
    setMissing(null);
    await p.onPick(path);
    p.onClose();
  });
  const onKey = (e: KeyboardEvent) => {
    if (e.key === "Escape") { e.preventDefault(); p.onClose(); }
    else if (e.key === "Enter") { e.preventDefault(); if (missing()) void createAndOpen(); else void go(); }
    else if (e.key === "ArrowDown") { e.preventDefault(); setAt((i) => Math.min(rows().length - 1, i + 1)); }
    else if (e.key === "ArrowUp") { e.preventDefault(); setAt((i) => Math.max(0, i - 1)); }
    e.stopPropagation();
  };
  onMount(() => { input.focus(); window.addEventListener("keydown", onKey, true); });
  onCleanup(() => window.removeEventListener("keydown", onKey, true));
  return (
    <div class="veil" onClick={p.onClose}>
      <div class="dlgbox wiz" onClick={(e) => e.stopPropagation()}>
        <div class="dlgt">▞ OPEN REPOSITORY <span class="crumb"><span>› {p.where ? `on ${p.where}` : "a known repo, or a folder path"}</span></span></div>
        <input ref={input} class="arc-q wide" placeholder={p.remote ? "search its folders… or a path on that machine (/home/…)" : "search known repos… or paste a path (C:\\Workspace\\…)"} value={q()} onInput={(e) => { setQ(e.currentTarget.value); setAt(0); setMissing(null); setError(null); }} />
        <div class="wiz-list">
          <Show when={looksLikePath()} fallback={
            <Show when={rows().length} fallback={<div class="hk-empty">▞ {p.loading ? "ASKING THE SERVER FOR ITS FOLDERS…" : p.known.length ? "EVERYTHING KNOWN IS OPEN — paste a path" : p.remote ? "NO KNOWN FOLDERS ON THAT MACHINE — type a path there" : "NO KNOWN REPOS — paste a path"}</div>}>
              <For each={rows()}>{(r, i) => (
                <button class={`cm-opt ${i() === at() ? "cur" : ""}`} disabled={busy()} onMouseMove={() => setAt(i())} onClick={() => void open(r.root)}>
                  <span class="n">{i() < 9 ? i() + 1 : ""}</span><span class="lb">{r.name}<span class="hint">{r.root}</span></span>
                </button>
              )}</For>
            </Show>
          }>
            <button class="cm-opt cur" disabled={busy()} onClick={() => void go()}><span class="n">↵</span><span class="lb">open folder<span class="hint">{q().trim()}</span></span></button>
          </Show>
        </div>
        <Show when={missing()}>{(path) => (
          <div class="repo-missing">
            <span>Folder <b>{path()}</b> does not exist{p.where ? ` on ${p.where}` : ""}.</span>
            <Show when={p.create}><button class="abtn ok" disabled={busy()} onClick={createAndOpen}>{busy() ? "CREATING…" : "CREATE FOLDER · OPEN"}</button></Show>
          </div>
        )}</Show>
        <Show when={error()}>{(e) => <div class="repo-error">{e()}</div>}</Show>
        <div class="dlga"><span class="micro">↑↓ · Enter · Esc</span><span class="sp" /><button class="abtn" onClick={p.onClose}>CANCEL</button><button class="abtn ok" disabled={busy()} onClick={() => void go()}>{busy() ? "…" : "OPEN"}</button></div>
      </div>
    </div>
  );
}
