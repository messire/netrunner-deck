import { For, Show, createSignal } from "solid-js";
import type { ArchiveEntry } from "../api";
import { describe, nameOf, resumable, shortId, sorted, when, worktreeRemovable } from "../archive";

/**
 * ARCHIVE — cold storage of closed spaces. One row per archived space: id,
 * name, description (written by a shadow `claude -p`), when, and the actions
 * RESTORE / DELETE / DELETE + WORKTREE / REWRITE. Destructive actions arm on
 * the first click and fire on the second within 3 s.
 */
export default function ArchiveView(p: {
  entries: ArchiveEntry[] | undefined;
  busy: string | null; // entry id with an action in flight
  error: string | null;
  onRestore: (id: string) => void;
  onDelete: (id: string, removeWorktree: boolean) => void;
  onSummarize: (id: string) => void;
  onRefresh: () => void;
}) {
  const [armed, setArmed] = createSignal<string | null>(null);
  let disarm: number | undefined;
  const arm = (key: string, fire: () => void) => {
    if (armed() === key) { setArmed(null); clearTimeout(disarm); fire(); return; }
    setArmed(key);
    clearTimeout(disarm);
    disarm = window.setTimeout(() => setArmed(null), 3000);
  };
  const rows = () => sorted(p.entries ?? []);
  return (
    <section class="archive">
      <div class="arc-head">
        <span class="plate">Archive · cold storage</span>
        <span class="micro">closed spaces that can come back · {rows().length} stored</span>
        <span class="sp" />
        <Show when={p.error}>{(m) => <span class="micro toast">✕ {m()}</span>}</Show>
        <button class="abtn" onClick={p.onRefresh}>↻ REFRESH</button>
      </div>
      <Show when={rows().length} fallback={<div class="arc-empty">▞ NOTHING IN STORAGE — right-click a space → CLOSE · ARCHIVE</div>}>
        <table class="arc">
          <thead>
            <tr><th>ID</th><th>NAME</th><th>DESCRIPTION</th><th>SESSIONS</th><th>STORED</th><th>STATUS</th><th></th></tr>
          </thead>
          <tbody>
            <For each={rows()}>
              {(e) => {
                const name = nameOf(e);
                const d = describe(e);
                const busy = () => p.busy === e.id;
                const key = (k: string) => `${e.id}:${k}`;
                return (
                  <tr class={e.status === "restored" ? "dim" : ""}>
                    <td class="mono">{shortId(e)}</td>
                    <td><div class="nm">{name.title}</div><div class="micro">{name.sub}</div></td>
                    <td class={`desc ${d.state}`}>{d.text}</td>
                    <td class="mono">{resumable(e)}<span class="m">/{e.sessions.length}</span></td>
                    <td class="mono">{when(e.archived_at_ms)}</td>
                    <td class="mono">{e.status === "restored" ? `RESTORED${e.restored_workspace_id ? ` · ${e.restored_workspace_id}` : ""}` : "ARCHIVED"}</td>
                    <td class="acts">
                      <button class="abtn ok" disabled={busy()} onClick={() => p.onRestore(e.id)} title={resumable(e) ? `workspace + claude --resume ×${resumable(e)}` : "reopen the folder only"}>
                        {busy() ? "…" : "RESTORE"}
                      </button>
                      <button class={`abtn ${armed() === key("del") ? "armed" : "danger"}`} disabled={busy()} onClick={() => arm(key("del"), () => p.onDelete(e.id, false))}>
                        {armed() === key("del") ? "CONFIRM" : "DELETE"}
                      </button>
                      <Show when={worktreeRemovable(e)}>
                        <button class={`abtn ${armed() === key("wt") ? "armed" : "danger"}`} disabled={busy()} title={`git worktree remove --force ${e.cwd}`}
                                onClick={() => arm(key("wt"), () => p.onDelete(e.id, true))}>
                          {armed() === key("wt") ? "CONFIRM" : "+ WORKTREE"}
                        </button>
                      </Show>
                      <Show when={e.summary_state !== "none" && e.summary_state !== "pending"}>
                        <button class="abtn" disabled={busy()} onClick={() => p.onSummarize(e.id)} title="write the description again">REWRITE</button>
                      </Show>
                    </td>
                  </tr>
                );
              }}
            </For>
          </tbody>
        </table>
      </Show>
    </section>
  );
}
