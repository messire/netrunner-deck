import { For, Show, createEffect, createMemo, createSignal, on, onCleanup, onMount } from "solid-js";
import { api } from "../api";
import { baseName, busyOf, choiceForBranch, everything, planOf, removalText, type Choice, type Plan, type TBranch, type TRepo } from "../transfer";
import type { Step } from "../jobs";

/** What the dialog shows while a transfer runs (owned by App, so it outlives the dialog). */
export interface TransferState { step: Step; error?: string | null; done?: string | null }

/**
 * COPY TO / MOVE TO another server: a repository (everything, or the
 * chosen branches with their sessions), a branch (it and all or some of
 * its sessions) or one session (its repository, branch and session; no
 * question). The work runs in App and goes on when this closes.
 */
export default function TransferDialog(p: {
  mode: "copy" | "move";
  scope: "repo" | "branch" | "session";
  repo: TRepo;
  /** The branch (scope branch / session). */
  branch?: TBranch;
  /** The session's pane (scope session). */
  session?: string;
  from: { id: string; name: string };
  to: { id: string; name: string };
  state: TransferState | null;
  onStart: (plan: Plan, path: string) => void;
  onClose: () => void;
}) {
  const [all, setAll] = createSignal(true);
  const [branches, setBranches] = createSignal<Set<string>>(new Set(p.branch ? [p.branch.workspaceId] : []));
  const [sessions, setSessions] = createSignal<Set<string>>(new Set(p.session ? [p.session] : []));
  const choice = createMemo((): Choice => {
    if (p.scope === "session" && p.branch) return choiceForBranch(p.branch, p.session);
    if (p.scope === "branch" && p.branch) return all() ? choiceForBranch(p.branch) : { whole: false, branches: new Set([p.branch.workspaceId]), sessions: sessions() };
    return all() ? everything() : { whole: false, branches: branches(), sessions: sessions() };
  });
  const plan = createMemo(() => planOf(p.repo, choice()));
  const busy = () => busyOf(plan());
  const [name, setName] = createSignal(baseName(p.repo.root));
  const path = () => `~/${name().trim()}`;
  /** The folder is taken on the target: another name is needed. */
  const [taken, setTaken] = createSignal<string | null>(null);
  let timer: number | undefined;
  createEffect(on(name, (n) => {
    clearTimeout(timer);
    setTaken(null);
    if (!n.trim()) return;
    timer = window.setTimeout(() => {
      api.folderState(p.to.id, `~/${n.trim()}`).then((st) => { if (name() === n && st.exists) setTaken(`${st.path} is already taken on ${p.to.name} — pick another name`); }).catch(() => {});
    }, 200);
  }));
  onCleanup(() => clearTimeout(timer));
  const nothing = () => !plan().worktrees.length;
  const running = () => !!p.state && !p.state.error && !p.state.done;
  const toggle = (set: (f: (s: Set<string>) => Set<string>) => void, id: string) => set((s) => { const n = new Set(s); if (n.has(id)) n.delete(id); else n.add(id); return n; });
  const toggleBranch = (b: TBranch) => {
    const on = branches().has(b.workspaceId);
    toggle(setBranches, b.workspaceId);
    // Unticking a branch unticks its sessions too.
    if (on) setSessions((s) => { const n = new Set(s); for (const x of b.sessions) n.delete(x.paneId); return n; });
  };
  const toggleSession = (b: TBranch, pane: string) => {
    toggle(setSessions, pane);
    setBranches((s) => new Set(s).add(b.workspaceId));
  };
  const verb = () => (p.mode === "move" ? "MOVE" : "COPY");
  const start = () => { if (!running() && !busy() && !taken() && !nothing() && name().trim()) p.onStart(plan(), path()); };
  const onKey = (e: KeyboardEvent) => { if (e.key === "Escape") { e.preventDefault(); e.stopPropagation(); p.onClose(); } };
  onMount(() => window.addEventListener("keydown", onKey, true));
  onCleanup(() => window.removeEventListener("keydown", onKey, true));

  const Session = (q: { b: TBranch; s: TBranch["sessions"][number] }) => (
    <label class={`xfer-row sess ${q.s.sessionId ? "" : "off"}`} title={q.s.sessionId ? q.s.cwd : "no conversation yet — nothing to transfer"}>
      <input type="checkbox" disabled={!q.s.sessionId || running()} checked={sessions().has(q.s.paneId)} onChange={() => toggleSession(q.b, q.s.paneId)} />
      <span class="nm">{q.s.label}</span>
      <Show when={q.s.status === "working" || q.s.status === "blocked"}><span class="tag b">BUSY</span></Show>
    </label>
  );

  return (
    <div class="veil" onClick={p.onClose}>
      <div class="dlgbox wiz xfer" onClick={(e) => e.stopPropagation()}>
        <div class="dlgt">▞ {verb()} TO {p.to.name.toUpperCase()} <span class="crumb"><span>› {p.repo.label}{p.branch && p.scope !== "repo" ? ` / ${p.branch.branch}` : ""}{p.scope === "session" ? ` / ${p.branch?.sessions.find((s) => s.paneId === p.session)?.label ?? "session"}` : ""}</span></span></div>
        <Show when={p.scope !== "session"}>
          <div class="xfer-mode">
            <label><input type="radio" name="xfer-all" checked={all()} disabled={running()} onChange={() => setAll(true)} /> Transfer everything</label>
            <label><input type="radio" name="xfer-all" checked={!all()} disabled={running()} onChange={() => setAll(false)} /> Transfer selected</label>
          </div>
          <div class={`xfer-list ${all() ? "dim" : ""}`} inert={all() || running()}>
            <Show when={p.scope === "repo"} fallback={
              <For each={p.branch?.sessions ?? []} fallback={<div class="hk-empty">no sessions in this branch — only the branch goes</div>}>{(s) => <Session b={p.branch!} s={s} />}</For>
            }>
              <For each={p.repo.branches}>{(b) => (
                <>
                  <label class="xfer-row br">
                    <input type="checkbox" checked={branches().has(b.workspaceId)} onChange={() => toggleBranch(b)} />
                    <span class="nm">{b.branch}</span>
                    <Show when={b.root}><span class="badge root">root</span></Show>
                    <span class="micro path">{b.path}</span>
                  </label>
                  <For each={b.sessions}>{(s) => <Session b={b} s={s} />}</For>
                </>
              )}</For>
            </Show>
          </div>
        </Show>
        <div class="clone-line xfer-target">
          <span>Into</span>
          <span class="mono">~/</span>
          <input class="arc-q clone-target" value={name()} disabled={running()} spellcheck={false} onInput={(e) => setName(e.currentTarget.value)} />
          <span>on <b>{p.to.name}</b></span>
        </div>
        <div class="micro xfer-sum">
          {plan().worktrees.length} {plan().worktrees.length === 1 ? "branch" : "branches"} · {plan().sessions.length} {plan().sessions.length === 1 ? "session" : "sessions"} · full history, uncommitted and untracked changes (not ignored build output)
        </div>
        <Show when={p.mode === "move"}><div class="xfer-warn">{removalText(plan(), p.from.name, p.to.name)}</div></Show>
        <Show when={busy() || taken()}>{(m) => <div class="repo-error"><span>{m()}</span></div>}</Show>
        <Show when={p.state}>{(st) => (
          <div class="xfer-progress">
            <div class="seg"><i style={{ width: `${st().step.percent ?? 0}%` }} /></div>
            <span class="micro">{st().error ? "✕ stopped" : st().done ? "✓ done" : st().step.phase}</span>
          </div>
        )}</Show>
        <Show when={p.state?.error}>{(e) => <div class="repo-error"><span>{e()}</span></div>}</Show>
        <Show when={p.state?.done}>{(d) => <div class="xfer-done">{d()}</div>}</Show>
        <div class="dlga">
          <span class="micro">{running() ? "closing this leaves the transfer running" : "Esc"}</span><span class="sp" />
          <button class="abtn" onClick={p.onClose}>{p.state?.done || running() ? "CLOSE" : "CANCEL"}</button>
          <Show when={!p.state?.done}>
            <button class={`abtn ${p.mode === "move" ? "danger" : "ok"}`} disabled={running() || !!busy() || !!taken() || nothing() || !name().trim()} onClick={start}>
              {running() ? `${verb()}ING…` : p.state?.error ? `${verb()} AGAIN` : verb()}
            </button>
          </Show>
        </div>
      </div>
    </div>
  );
}
