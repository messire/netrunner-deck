import { For, Show, createMemo, createSignal, onMount } from "solid-js";
import type { ArchiveEntry, RepoGroup } from "../api";
import type { Source } from "../sources";
import ServerSections, { SourceTag } from "./ServerSections";
import { describe, resumable, shortId, when } from "../archive";
import { buildArchiveTree, deletePlan, groupRepos, moveRepo, type RepoNode, type WorktreeNode } from "../archive-tree";

/**
 * ARCHIVE — cold storage as a tree, like the Sessions tree: repository ›
 * worktree › session. Every Claude session on the picked server's machine
 * that is not open on that server (one section per server under ALL). Sessions: RESTORE / DELETE / WRITE. A worktree row deletes all
 * its sessions and, for a linked worktree, the checkout; a repository row
 * deletes every worktree under it (the repository's own checkout stays).
 * Repositories can be sorted into user groups (drag a repo onto a group
 * header, or use its MOVE menu); a group can be hidden with the eye.
 * Destructive buttons arm on the first click and fire on the second (3 s).
 */
const OPEN_KEY = "deck.archive.open";
function loadOpen(): Record<string, boolean> {
  try { return JSON.parse(localStorage.getItem(OPEN_KEY) ?? "{}"); } catch { return {}; }
}

export default function ArchiveView(p: {
  sources: Source<ArchiveEntry[]>[];
  /** `<server>|<entry id>` of the action in flight. */
  busy: string | null;
  error: string | null;
  onRestore: (server: string, id: string) => void;
  onDeleteMany: (server: string, plan: { id: string; removeWorktree: boolean }[]) => void;
  onSummarize: (server: string, id: string) => void;
  onRefresh: () => void;
  groups: RepoGroup[];
  onGroups: (groups: RepoGroup[]) => void;
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
  const treeOf = (entries: ArchiveEntry[] | undefined) => buildArchiveTree(entries ?? [], query());
  const trees = createMemo(() => p.sources.map((s) => treeOf(s.data)));
  // Two servers may hold the same repository: under ALL its fold state is per server.
  const keyOf = (server: string, id: string) => (p.sources.length > 1 ? `${server}:${id}` : id);
  const [dragging, setDragging] = createSignal<string | null>(null); // repo key being dragged
  const [over, setOver] = createSignal<string | null>(null); // group id (or "main") under the pointer
  const dropOn = (groupId: string | null) => {
    const key = dragging();
    setDragging(null); setOver(null);
    if (key) p.onGroups(moveRepo(p.groups, key, groupId));
  };
  // In-app prompt (the browser's window.prompt is the OS dialog, off-style).
  const [ask, setAsk] = createSignal<{ title: string; value: string; ok: (v: string) => void } | null>(null);
  const newGroup = () => setAsk({ title: "NEW GROUP", value: "", ok: (name) => p.onGroups([...p.groups, { id: `g${Date.now().toString(36)}`, name, repos: [], hidden: false }]) });
  const renameGroup = (g: RepoGroup) => setAsk({ title: `RENAME ${g.name.toUpperCase()}`, value: g.name, ok: (name) => { if (name !== g.name) p.onGroups(p.groups.map((x) => (x.id === g.id ? { ...x, name } : x))); } });
  const toggleHidden = (g: RepoGroup) => p.onGroups(p.groups.map((x) => (x.id === g.id ? { ...x, hidden: !x.hidden } : x)));
  const removeGroup = (g: RepoGroup) => p.onGroups(p.groups.filter((x) => x.id !== g.id)); // its repos fall back to main
  const total = () => p.sources.reduce((n, s) => n + (s.data ?? []).length, 0);
  const shown = () => trees().reduce((n, t) => n + t.reduce((m, r) => m + r.sessions, 0), 0);
  const repoCount = () => trees().reduce((n, t) => n + t.length, 0);
  const busy = () => p.busy !== null;

  const Danger = (q: { k: string; label: string; title?: string; onFire: () => void }) => (
    <button class={`abtn ${armed() === q.k ? "armed" : "danger"}`} disabled={busy()} title={q.title} onClick={(e) => { e.stopPropagation(); arm(q.k, q.onFire); }}>
      {armed() === q.k ? "CONFIRM" : q.label}
    </button>
  );

  const SessionRow = (q: { e: ArchiveEntry; src: string }) => {
    const d = describe(q.e);
    const own = () => p.busy === `${q.src}|${q.e.id}`;
    const tabNames = (e: ArchiveEntry) => [...new Set(e.sessions.map((x) => x.tab_label).filter(Boolean))].join(" · ");
    return (
      <div class={`arow sess ${q.e.status === "restored" ? "dim" : ""}`}>
        <span class="tw" />
        <span class="mono id" title={q.e.id}>{shortId(q.e)}</span>
        <span class={`desc ${d.state}`} title={q.e.cwd}>
          <Show when={tabNames(q.e)}>{(n) => <span class="tabname" title="tab name(s), restored with the session">{n()}</span>}</Show>
          {d.text}
        </span>
        <span class="mono cnt" title="resumable / panes">{resumable(q.e)}<span class="m">/{q.e.sessions.length}</span></span>
        <span class="mono when">{when(q.e.last_active_ms)}</span>
        <span class="mono stat">{q.e.status === "restored" ? `RESTORED${q.e.restored_workspace_id ? ` · ${q.e.restored_workspace_id}` : ""}` : "CLOSED"}</span>
        <span class="acts">
          <button class="abtn ok" disabled={busy()} onClick={() => p.onRestore(q.src, q.e.id)} title={resumable(q.e) ? `workspace + claude --resume ×${resumable(q.e)}` : "reopen the folder only"}>{own() ? "…" : "RESTORE"}</button>
          <Danger k={`s:${q.src}:${q.e.id}`} label="DELETE" title="forget it and delete the transcript(s) under ~/.claude/projects" onFire={() => p.onDeleteMany(q.src, [{ id: q.e.id, removeWorktree: false }])} />
          <Show when={q.e.summary_state !== "pending"}>
            <button class="abtn" disabled={busy()} onClick={() => p.onSummarize(q.src, q.e.id)} title="shadow claude -p writes a 1–2 sentence description">{q.e.summary_state === "ready" ? "REWRITE" : "WRITE"}</button>
          </Show>
        </span>
      </div>
    );
  };

  const WorktreeRow = (q: { r: RepoNode; w: WorktreeNode; src: string }) => {
    const k = () => keyOf(q.src, q.w.id);
    return (
    <>
      <div class={`arow wt ${isOpen(k(), "worktree") ? "open" : ""}`} onClick={() => toggle(k(), "worktree")}>
        <span class="tw">{isOpen(k(), "worktree") ? "▾" : "▸"}</span>
        <span class={`gem ${q.w.root ? "root" : ""}`} />
        <span class="nm wtn">{q.w.name}</span>
        <Show when={q.w.root}><span class="src">ROOT</span></Show>
        <Show when={q.w.linked}><span class="src herdr">WORKTREE</span></Show>
        <span class="micro path" title={q.w.path}>{q.w.path}</span>
        <span class="sp" />
        <span class="mono cnt">{q.w.sessions.length}</span>
        <span class="mono when">{when(q.w.lastActive)}</span>
        <span class="acts">
          <Danger k={`w:${q.src}:${q.w.id}`} label={q.w.linked ? "DELETE WORKTREE" : "DELETE SESSIONS"}
                  title={q.w.linked ? `all ${q.w.sessions.length} session(s) + git worktree remove --force ${q.w.path}` : `all ${q.w.sessions.length} session(s); the checkout stays`}
                  onFire={() => p.onDeleteMany(q.src, deletePlan(q.w))} />
        </span>
      </div>
      <Show when={isOpen(k(), "worktree")}>
        <For each={q.w.sessions}>{(s) => <SessionRow e={s.entry} src={q.src} />}</For>
      </Show>
    </>
    );
  };

  const PromptBox = (q: { title: string; value: string; ok: (v: string) => void }) => {
    let input!: HTMLInputElement;
    onMount(() => { input.focus(); input.select(); });
    // Read the props before closing: once `ask` is null the <Show> accessor behind them is gone.
    const submit = () => { const v = input.value.trim(); const ok = q.ok; setAsk(null); if (v) ok(v); };
    return (
      <div class="veil" onClick={() => setAsk(null)}>
        <div class="dlgbox" onClick={(e) => e.stopPropagation()} onKeyDown={(e) => { if (e.key === "Enter") submit(); if (e.key === "Escape") setAsk(null); }}>
          <div class="dlgt">▞ {q.title}</div>
          <input ref={input} class="arc-q wide" value={q.value} placeholder="group name" />
          <div class="dlga">
            <button class="abtn" onClick={() => setAsk(null)}>CANCEL</button>
            <button class="abtn ok" onClick={submit}>OK</button>
          </div>
        </div>
      </div>
    );
  };

  return (
    <section class="archive">
      <Show when={ask()}>{(q) => <PromptBox title={q().title} value={q().value} ok={q().ok} />}</Show>
      <div class="arc-head">
        <span class="plate">Archive · cold storage</span>
        <SourceTag sources={p.sources} />
        <span class="micro">closed sessions · {shown()}{query() ? ` of ${total()}` : ""} in {repoCount()} repos</span>
        <input class="arc-q" placeholder="filter: repo, branch, words from the prompt…" value={query()} onInput={(e) => setQuery(e.currentTarget.value)} />
        <span class="sp" />
        <Show when={p.error}>{(m) => <span class="micro toast">✕ {m()}</span>}</Show>
        <button class="abtn" onClick={newGroup} title="a group of repositories; drag repos onto its header">+ GROUP</button>
        <button class="abtn" onClick={p.onRefresh}>↻ REFRESH</button>
      </div>
      <div class="arow head">
        <span class="tw" /><span class="mono id">ID</span><span class="desc">DESCRIPTION</span><span class="mono cnt">SESS</span><span class="mono when">LAST ACTIVE</span><span class="mono stat">STATUS</span><span class="acts" />
      </div>
      <ServerSections view="archive" sources={p.sources} count={(s) => (s.data ?? []).filter((e) => e.status === "closed").length}>{(src) => {
      const tree = createMemo(() => treeOf(src.data));
      const sections = createMemo(() => groupRepos(tree(), p.groups));
      return (
      <Show when={src.data} fallback={<div class="arc-empty">▞ SCANNING…</div>}>
      <Show when={tree().length} fallback={<div class="arc-empty">{query() ? "▞ NOTHING MATCHES" : "▞ NOTHING IN STORAGE — every Claude session there is open"}</div>}>
        <For each={sections()}>
          {(sec) => {
            const gid = () => sec.group?.id ?? "main";
            const hidden = () => sec.group?.hidden ?? false;
            const dropZone = {
              onDragOver: (e: DragEvent) => { if (dragging()) { e.preventDefault(); setOver(gid()); } },
              onDragLeave: () => { if (over() === gid()) setOver(null); },
              onDrop: (e: DragEvent) => { e.preventDefault(); dropOn(sec.group?.id ?? null); },
            };
            return (
              <Show when={sec.group || sec.repos.length || p.groups.length}>
                <div class={`arow grp ${over() === gid() ? "over" : ""} ${hidden() ? "hid" : ""}`} {...dropZone}>
                  <span class="tw">{hidden() ? "◌" : "◈"}</span>
                  <span class="nm gn">{sec.group?.name ?? "MAIN"}</span>
                  <span class="micro">{sec.repos.length} repo{sec.repos.length === 1 ? "" : "s"}{hidden() ? " · hidden" : ""}{dragging() ? " · drop here" : ""}</span>
                  <span class="sp" />
                  <Show when={sec.group}>{(g) => (
                    <span class="acts">
                      <button class="abtn eye" title={g().hidden ? "show this group" : "hide this group"} onClick={() => toggleHidden(g())}>{g().hidden ? "◌ HIDDEN" : "◉ SHOWN"}</button>
                      <button class="abtn" title="rename" onClick={() => renameGroup(g())}>RENAME</button>
                      <button class="abtn danger" title="remove the group; its repos go back to MAIN" onClick={() => removeGroup(g())}>✕</button>
                    </span>
                  )}</Show>
                </div>
                <Show when={!hidden()}>
                  <For each={sec.repos}>
                    {(r) => (
                      <>
                        <div class={`arow repo ${isOpen(keyOf(src.id, r.id), "repo") ? "open" : ""} ${dragging() === r.id ? "drag" : ""}`} onClick={() => toggle(keyOf(src.id, r.id), "repo")}
                             draggable onDragStart={(e) => { setDragging(r.id); e.dataTransfer?.setData("text/plain", r.id); }} onDragEnd={() => { setDragging(null); setOver(null); }}>
                          <span class="tw">{isOpen(keyOf(src.id, r.id), "repo") ? "▾" : "▸"}</span>
                          <span class="nm rn">{r.name}</span>
                          <span class="micro">{r.worktrees.length} worktree{r.worktrees.length === 1 ? "" : "s"} · {r.sessions} session{r.sessions === 1 ? "" : "s"}</span>
                          <span class="sp" />
                          <span class="mono when">{when(r.lastActive)}</span>
                          <span class="acts" onClick={(e) => e.stopPropagation()}>
                            <select class="asel" title="move to a group" value={sec.group?.id ?? ""} onChange={(e) => p.onGroups(moveRepo(p.groups, r.id, e.currentTarget.value || null))}>
                              <option value="">MAIN</option>
                              <For each={p.groups}>{(g) => <option value={g.id}>{g.name.toUpperCase()}</option>}</For>
                            </select>
                            <Danger k={`r:${src.id}:${r.id}`} label="DELETE REPO" title={`every worktree and session under ${r.name}; the repository's own checkout stays`} onFire={() => p.onDeleteMany(src.id, deletePlan(r))} />
                          </span>
                        </div>
                        <Show when={isOpen(keyOf(src.id, r.id), "repo")}>
                          <For each={r.worktrees}>{(w) => <WorktreeRow r={r} w={w} src={src.id} />}</For>
                        </Show>
                      </>
                    )}
                  </For>
                </Show>
              </Show>
            );
          }}
        </For>
      </Show>
      </Show>
      );
      }}</ServerSections>
    </section>
  );
}
