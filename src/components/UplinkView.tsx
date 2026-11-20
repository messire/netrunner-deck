import { For, Show, createSignal } from "solid-js";
import type { Update, UplinkCommit } from "../api";

/** UPLINK: what each `git pull` brought into which checkout — PR numbers with links, titles, authors. */
interface Props { updates: Update[] | undefined; error: string | null; onRemove: (id: string) => void; onRefresh: () => void }

const when = (ms: number) => new Date(ms).toLocaleString([], { day: "2-digit", month: "2-digit", hour: "2-digit", minute: "2-digit" });

export default function UplinkView(p: Props) {
  const [query, setQuery] = createSignal("");
  const shown = () => {
    const words = query().toLowerCase().split(/\s+/).filter(Boolean);
    const all = p.updates ?? [];
    if (!words.length) return all;
    return all.filter((u) => words.every((w) => `${u.repo} ${u.branch} ${u.path} ${u.commits.map((c) => `#${c.pr ?? ""} ${c.title} ${c.author}`).join(" ")}`.toLowerCase().includes(w)));
  };
  return (
    <section class="archive uplink">
      <div class="arc-head">
        <span class="plate">Uplink · pulled changes</span>
        <span class="micro">what each PULL brought into a checkout · {(p.updates ?? []).length} pulls logged</span>
        <input class="arc-q" placeholder="filter: repo, branch, #PR, words from a title…" value={query()} onInput={(e) => setQuery(e.currentTarget.value)} />
        <span class="sp" />
        <Show when={p.error}>{(m) => <span class="micro toast">✕ {m()}</span>}</Show>
        <button class="abtn" onClick={p.onRefresh}>↻ REFRESH</button>
      </div>
      <Show when={shown().length} fallback={<div class="arc-empty">{query() ? "▞ NOTHING MATCHES" : "▞ NO PULLS YET — right-click a worktree in SESSIONS → PULL · UPDATE"}</div>}>
        <For each={shown()}>{(u) => (
          <div class={`upd ${u.seen ? "" : "fresh"}`}>
            <div class="arow repo upd-head">
              <span class="gem root" />
              <span class="nm rn">{u.repo}</span>
              <span class="nm wtn">{u.branch}</span>
              <span class="path mono">{u.path}</span>
              <span class="sp" />
              <span class="mono range">{u.from} → {u.to}</span>
              <span class="mono cnt">{u.commits.length}</span>
              <span class="mono when">{when(u.at_ms)}</span>
              <button class="abtn" onClick={() => p.onRemove(u.id)} title="forget this entry">✕</button>
            </div>
            <Show when={u.commits.length} fallback={<div class="hnone">already up to date — nothing came in</div>}>
              <For each={u.commits}>{(c) => <CommitRow c={c} />}</For>
            </Show>
          </div>
        )}</For>
      </Show>
    </section>
  );
}

function CommitRow(p: { c: UplinkCommit }) {
  const c = p.c;
  return (
    <div class="arow sess upd-row">
      <span class="mono pr">
        <Show when={c.url} fallback={<span>{c.pr ? `#${c.pr}` : c.sha.slice(0, 7)}</span>}>{(u) => <a href={u()} title={u()}>{c.pr ? `#${c.pr}` : c.sha.slice(0, 7)}</a>}</Show>
      </span>
      <span class="desc">{c.title}</span>
      <span class="mono author">{c.author}</span>
      <span class="mono when">{when(c.at_ms)}</span>
    </div>
  );
}
