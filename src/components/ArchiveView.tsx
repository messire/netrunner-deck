import { For, Show, createMemo, createSignal } from "solid-js";
import type { ArchiveEntry } from "../api";
import { describe, resumable, shortId, when } from "../archive";
import { buildArchiveTree, deletePlan, type RepoNode, type WorktreeNode } from "../archive-tree";

/**
 * ARCHIVE — cold storage as a tree, like the Sessions tree: repository ›
 * worktree › session. Every Claude session on this computer that is not open
 * in Herdr. Sessions: RESTORE / DELETE / WRITE. A worktree row deletes all
 * its sessions and, for a linked worktree, the checkout; a repository row
 * deletes every worktree under it (the repository's own checkout stays).
 * Destructive buttons arm on the first click and fire on the second (3 s).
 */
const OPEN_KEY = "deck.archive.open";
function loadOpen(): Record<string, boolean> {
  try { return JSON.parse(localStorage.getItem(OPEN_KEY) ?? "{}"); } catch { return {}; }
}

export default function ArchiveView(p: {
  entries: ArchiveEntry[] | undefined;
  busy: string | null;
  error: string | null;
  onRestore: (id: string) => void;
  onDeleteMany: (plan: { id: string; removeWorktree: boolean }[]) => void;
  onSummarize: (id: string) => void;
  onRefresh: () => void;
}) {
  const [query, setQuery] = createSignal("");
  const [open, setOpen] = createSignal<Record<string, boolean>>(loadOpen());
  // Repos start open, worktrees closed; a search opens everything that matched.
  const isOpen = (id: string, kind: "repo" | "worktree") => (query() ? true : open()[id] ?? kind === "repo");
  const toggle = (id: string, kind: "repo" | "worktree") => {
    const next = { ...open(), [id]: !isOpen(id, kind) };
    setOpen(next);
    try { localStorage.setItem(OPEN_KEY, JSON.stringify(next)); } catch { /* per-viewer convenience only */ }
  };
  const [armed, setArmed] = createSignal<string | null>(null);
  let disarm: number | undefined;
  const arm = (key: string, fire: () => void) => {
    if (armed() === key) { setArmed(null); clearTimeout(disarm); fire(); return; }
    setArmed(key);
    clearTimeout(disarm);
    disarm = window.setTimeout(() => setArmed(null), 3000);
  };
  const tree = createMemo(() => buildArchiveTree(p.entries ?? [], query()));
  const total = () => (p.entries ?? []).length;
  const shown = () => tree().reduce((n, r) => n + r.sessions, 0);
  const busy = () => p.busy !== null;

  const Danger = (q: { k: string; label: string; title?: string; onFire: () => void }) => (
    <button class={`abtn ${armed() === q.k ? "armed" : "danger"}`} disabled={busy()} title={q.title} onClick={(e) => { e.stopPropagation(); arm(q.k, q.onFire); }}>
      {armed() === q.k ? "CONFIRM" : q.label}
    </button>
  );

  const SessionRow = (q: { e: ArchiveEntry }) => {
    const d = describe(q.e);
    const own = () => p.busy === q.e.id;
    return (
      <div class={`arow sess ${q.e.status === "restored" ? "dim" : ""}`}>
        <span class="tw" />
        <span class="mono id" title={q.e.id}>{shortId(q.e)}</span>
        <span class={`desc ${d.state}`} title={q.e.cwd}>{d.text}</span>
        <span class="mono cnt" title="resumable / panes">{resumable(q.e)}<span class="m">/{q.e.sessions.length}</span></span>
        <span class="mono when">{when(q.e.last_active_ms)}</span>
        <span class="mono st">{q.e.status === "restored" ? `RESTORED${q.e.restored_workspace_id ? ` · ${q.e.restored_workspace_id}` : ""}` : "CLOSED"}</span>
        <span class="acts">
          <button class="abtn ok" disabled={busy()} onClick={() => p.onRestore(q.e.id)} title={resumable(q.e) ? `workspace + claude --resume ×${resumable(q.e)}` : "reopen the folder only"}>{own() ? "…" : "RESTORE"}</button>
          <Danger k={`s:${q.e.id}`} label="DELETE" title="forget it and delete the transcript(s) under ~/.claude/projects" onFire={() => p.onDeleteMany([{ id: q.e.id, removeWorktree: false }])} />
          <Show when={q.e.summary_state !== "pending"}>
            <button class="abtn" disabled={busy()} onClick={() => p.onSummarize(q.e.id)} title="shadow claude -p writes a 1–2 sentence description">{q.e.summary_state === "ready" ? "REWRITE" : "WRITE"}</button>
          </Show>
        </span>
      </div>
    );
  };

  const WorktreeRow = (q: { r: RepoNode; w: WorktreeNode }) => (
    <>
      <div class={`arow wt ${isOpen(q.w.id, "worktree") ? "open" : ""}`} onClick={() => toggle(q.w.id, "worktree")}>
        <span class="tw">{isOpen(q.w.id, "worktree") ? "▾" : "▸"}</span>
        <span class={`st2 ${q.w.root ? "root" : ""}`} />
        <span class="nm wtn">{q.w.name}</span>
        <Show when={q.w.root}><span class="src">ROOT</span></Show>
        <Show when={q.w.linked}><span class="src herdr">WORKTREE</span></Show>
        <span class="micro path" title={q.w.path}>{q.w.path}</span>
        <span class="sp" />
        <span class="mono cnt">{q.w.sessions.length}</span>
        <span class="mono when">{when(q.w.lastActive)}</span>
        <span class="acts">
          <Danger k={`w:${q.w.id}`} label={q.w.linked ? "DELETE WORKTREE" : "DELETE SESSIONS"}
                  title={q.w.linked ? `all ${q.w.sessions.length} session(s) + git worktree remove --force ${q.w.path}` : `all ${q.w.sessions.length} session(s); the checkout stays`}
                  onFire={() => p.onDeleteMany(deletePlan(q.w))} />
        </span>
      </div>
      <Show when={isOpen(q.w.id, "worktree")}>
        <For each={q.w.sessions}>{(s) => <SessionRow e={s.entry} />}</For>
      </Show>
    </>
  );

  return (
    <section class="archive">
      <div class="arc-head">
        <span class="plate">Archive · cold storage</span>
        <span class="micro">closed sessions on this machine · {shown()}{query() ? ` of ${total()}` : ""} in {tree().length} repos</span>
        <input class="arc-q" placeholder="filter: repo, branch, words from the prompt…" value={query()} onInput={(e) => setQuery(e.currentTarget.value)} />
        <span class="sp" />
        <Show when={p.error}>{(m) => <span class="micro toast">✕ {m()}</span>}</Show>
        <button class="abtn" onClick={p.onRefresh}>↻ REFRESH</button>
      </div>
      <div class="arow head">
        <span class="tw" /><span class="mono id">ID</span><span class="desc">DESCRIPTION</span><span class="mono cnt">SESS</span><span class="mono when">LAST ACTIVE</span><span class="mono st">STATUS</span><span class="acts" />
      </div>
      <Show when={tree().length} fallback={<div class="arc-empty">{query() ? "▞ NOTHING MATCHES" : "▞ NOTHING IN STORAGE — every Claude session on this machine is open in Herdr"}</div>}>
        <For each={tree()}>
          {(r) => (
            <>
              <div class={`arow repo ${isOpen(r.id, "repo") ? "open" : ""}`} onClick={() => toggle(r.id, "repo")}>
                <span class="tw">{isOpen(r.id, "repo") ? "▾" : "▸"}</span>
                <span class="nm rn">{r.name}</span>
                <span class="micro">{r.worktrees.length} worktree{r.worktrees.length === 1 ? "" : "s"} · {r.sessions} session{r.sessions === 1 ? "" : "s"}</span>
                <span class="sp" />
                <span class="mono when">{when(r.lastActive)}</span>
                <span class="acts">
                  <Danger k={`r:${r.id}`} label="DELETE REPO" title={`every worktree and session under ${r.name}; the repository's own checkout stays`} onFire={() => p.onDeleteMany(deletePlan(r))} />
                </span>
              </div>
              <Show when={isOpen(r.id, "repo")}>
                <For each={r.worktrees}>{(w) => <WorktreeRow r={r} w={w} />}</For>
              </Show>
            </>
          )}
        </For>
      </Show>
    </section>
  );
}
