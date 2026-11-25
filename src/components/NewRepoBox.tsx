import { For, Show, createEffect, createMemo, createSignal, on, onCleanup, onMount } from "solid-js";
import { api, type MachineJob } from "../api";
import { filterItems } from "../quickhacks";
import { cloneLine, isGitHub, isGitUrl, repoNameOf } from "../transfer";
import { waitJob } from "../jobs";
import GitHubSignInCard from "./GitHubSignInCard";

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
  /** The server's id, for a clone on its machine (deck-server only: `cloneable`). */
  server?: string | null;
  cloneable?: boolean;
}) {
  const [q, setQ] = createSignal("");
  const [at, setAt] = createSignal(0);
  let input!: HTMLInputElement;
  const rows = createMemo(() => filterItems(p.known.filter((r) => !r.open).map((r) => ({ ...r, description: r.root })), q()));
  const looksLikePath = () => /^([a-zA-Z]:[\\/]|\/|\\\\|~)/.test(q().trim());
  /** On another machine anything typed that matches no known folder is a folder in the home there. */
  const asPath = () => looksLikePath() || (!!p.remote && !!q().trim() && rows().length === 0);
  /** Where the typed folder is on the server's machine, as it resolves it (inside the home there). */
  const [resolved, setResolved] = createSignal<{ path: string; exists: boolean; dir: boolean } | null>(null);
  let resolveTimer: number | undefined;
  createEffect(on(q, (text) => {
    clearTimeout(resolveTimer);
    setResolved(null);
    const v = text.trim();
    if (!p.remote || !p.check || !v || !asPath() || isGitUrl(v)) return;
    resolveTimer = window.setTimeout(() => {
      p.check!(v).then((st) => { if (q().trim() === v) setResolved(st); }).catch(() => {});
    }, 250);
  }));
  onCleanup(() => clearTimeout(resolveTimer));
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
    if (asPath()) {
      const v = q().trim();
      return run(async () => {
        const state = await p.check?.(v).catch((e) => { throw new Error(String(e).replace(/^Error: /, "").replace(/^the server refused deck\.fs\.\w+: /, "")); });
        if (state && !state.exists) { setMissing(state.path); return; }
        if (state && !state.dir) throw new Error(`${state.path} is a file, not a folder`);
        await p.onPick(state?.path ?? v);
        p.onClose();
      });
    }
    const r = rows()[at()];
    if (r) void open(r.root);
  };
  /** Creates `path` (default: the missing one) on the server's machine and opens it. */
  const createAndOpen = (target?: string) => run(async () => {
    const path = target ?? missing();
    if (!path || !p.create) return;
    const made = (await p.create(path)) as { path?: string } | undefined;
    setMissing(null);
    await p.onPick(made?.path ?? path);
    p.onClose();
  });
  // A pasted git address: clone it on the server's machine, then open it.
  const url = () => (isGitUrl(q()) ? q().trim() : null);
  const [target, setTarget] = createSignal<string | null>(null);
  const cloneTarget = () => target() ?? `~/${repoNameOf(url() ?? "")}`;
  const [cloning, setCloning] = createSignal<MachineJob | null>(null);
  /** The clone was refused for a github.com address: offer GitHub sign-in on the machine. */
  const [needGitHub, setNeedGitHub] = createSignal(false);
  let mounted = true;
  onCleanup(() => { mounted = false; });
  const clone = async () => {
    const u = url();
    if (!u || cloning()?.state === "running") return;
    if (!p.cloneable) { setError(`cloning needs deck-server on ${p.where || "that machine"}`); return; }
    setError(null); setNeedGitHub(false);
    try {
      const started = await api.machine<{ job: string; path: string }>(p.server ?? null, "deck.git.clone.start", { url: u, path: cloneTarget() });
      cloneJob = started.job;
      setCloning({ state: "running", phase: "Starting", path: started.path });
      // Not tied to the dialog: a clone that finishes after it closed still opens.
      const end = await waitJob(() => api.machine(p.server ?? null, "deck.git.clone.status", { job: started.job }), (s) => { if (mounted) setCloning(s); }, 500);
      if (end.state === "done") {
        await p.onPick(String(end.path ?? started.path));
        if (mounted) p.onClose();
        return;
      }
      if (!mounted) return;
      setCloning(null);
      if (end.state === "cancelled") return;
      if (end.auth && isGitHub(u)) setNeedGitHub(true);
      setError(String(end.message ?? "the clone failed"));
    } catch (e) {
      if (mounted) { setCloning(null); setError(String(e).replace(/^Error: /, "").replace(/^the server refused deck\.git\.[\w.]+: /, "")); }
    }
  };
  let cloneJob: string | null = null;
  const cancelClone = () => { if (cloneJob) void api.machine(p.server ?? null, "deck.git.clone.cancel", { job: cloneJob }).catch(() => {}); };
  const onKey = (e: KeyboardEvent) => {
    if (e.key === "Escape") { e.preventDefault(); p.onClose(); }
    else if (e.key === "Enter") { e.preventDefault(); if (url()) void clone(); else if (missing()) void createAndOpen(); else void go(); }
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
        <input ref={input} class="arc-q wide" placeholder={p.remote ? "search its folders… a folder in your home there… or paste a git URL" : "search known repos… a path (C:\\Workspace\\…) or a git URL"} value={q()} onInput={(e) => { setQ(e.currentTarget.value); setAt(0); setMissing(null); setError(null); setTarget(null); setNeedGitHub(false); }} />
        <div class="wiz-list">
          <Show when={!url()} fallback={
            <div class="clone-box">
              <div class="clone-line">
                <span>Clone into</span>
                <input class="arc-q clone-target" value={cloneTarget()} disabled={!!cloning()} spellcheck={false}
                  onInput={(e) => setTarget(e.currentTarget.value)} title="the folder on that machine (a new or empty one)" />
                <span>on <b>{p.where || "this server"}</b></span>
              </div>
              <Show when={cloning()}>{(c) => (
                <div class="clone-progress">
                  <div class="seg"><i style={{ width: `${c().percent ?? 4}%` }} /></div>
                  <span class="micro">{cloneLine(c())}</span>
                </div>
              )}</Show>
              <Show when={needGitHub()}>
                <GitHubSignInCard server={p.server ?? null} where={p.where} onDone={() => { setNeedGitHub(false); void clone(); }} onCancel={() => setNeedGitHub(false)} />
              </Show>
            </div>
          }>
          <Show when={asPath()} fallback={
            <Show when={rows().length} fallback={<div class="hk-empty">▞ {p.loading ? "ASKING THE SERVER FOR ITS FOLDERS…" : p.known.length ? "EVERYTHING KNOWN IS OPEN — paste a path" : p.remote ? "NO KNOWN FOLDERS ON THAT MACHINE — type a path there" : "NO KNOWN REPOS — paste a path"}</div>}>
              <For each={rows()}>{(r, i) => (
                <button class={`cm-opt ${i() === at() ? "cur" : ""}`} disabled={busy()} onMouseMove={() => setAt(i())} onClick={() => void open(r.root)}>
                  <span class="n">{i() < 9 ? i() + 1 : ""}</span><span class="lb">{r.name}<span class="hint">{r.root}</span></span>
                </button>
              )}</For>
            </Show>
          }>
            <button class="cm-opt cur" disabled={busy()} onClick={() => void go()}><span class="n">↵</span><span class="lb">open folder<span class="hint">{resolved()?.path ?? q().trim()}{resolved() && !resolved()!.exists ? " · not there yet" : ""}</span></span></button>
          </Show>
          </Show>
        </div>
        <Show when={missing()}>{(path) => (
          <div class="repo-missing">
            <span>Create folder <b>{path()}</b>{p.where ? ` on ${p.where}` : ""}?</span>
            <Show when={p.create}><button class="abtn ok" disabled={busy()} onClick={() => void createAndOpen()}>{busy() ? "CREATING…" : "CREATE · OPEN"}</button></Show>
          </div>
        )}</Show>
        <Show when={!needGitHub() && error()}>{(e) => (
          <div class="repo-error"><span>{e().replace(/^the server refused deck\.fs\.\w+: /, "")}</span></div>
        )}</Show>
        <div class="dlga"><span class="micro">↑↓ · Enter · Esc</span><span class="sp" /><button class="abtn" onClick={p.onClose}>CANCEL</button><Show when={url()} fallback={<button class="abtn ok" disabled={busy()} onClick={() => void go()}>{busy() ? "…" : "OPEN"}</button>}>
            <Show when={cloning()?.state === "running"}><button class="abtn" onClick={cancelClone}>STOP CLONE</button></Show>
            <button class="abtn ok" disabled={!!cloning()} onClick={() => void clone()}>{cloning() ? "CLONING…" : "CLONE · OPEN"}</button>
          </Show></div>
      </div>
    </div>
  );
}
