import { For, Show, createMemo } from "solid-js";
import { accountLabel, usageOf, type CswapList, type Tree, type AgentStatus } from "../api";

interface Props {
  tree?: Tree;
  accounts?: CswapList;
  selectedPane: string | null;
  onSelectPane: (paneId: string) => void;
  onSwitchAccount: (number: number) => void;
  onRefreshAccounts: () => void;
}

const statusClass = (s: AgentStatus) => (s === "working" ? "w" : s === "blocked" ? "b" : "");

export default function Sidebar(p: Props) {
  // Worktree children sit under their repo; everything else is a top-level space.
  const spaces = createMemo(() => {
    const t = p.tree;
    if (!t) return [];
    const groups = new Map<string, { label: string; items: typeof t.workspaces }>();
    const top: typeof t.workspaces = [];
    for (const w of t.workspaces) {
      if (w.worktree) {
        const key = w.worktree.repo_key;
        if (!groups.has(key)) groups.set(key, { label: w.worktree.repo_name, items: [] });
        groups.get(key)!.items.push(w);
      } else top.push(w);
    }
    return { top, groups: [...groups.values()] };
  });

  const panesOf = (workspaceId: string) => p.tree?.panes.filter((x) => x.workspace_id === workspaceId) ?? [];
  const agents = createMemo(() => {
    const list = [...(p.tree?.agents ?? [])];
    // Recency: working first, then blocked, then the rest (server order within groups).
    const rank: Record<string, number> = { working: 0, blocked: 1, idle: 2, done: 3, unknown: 4 };
    return list.sort((a, b) => (rank[a.agent_status] ?? 9) - (rank[b.agent_status] ?? 9));
  });
  const ctxOf = (tokens?: Record<string, string>) => {
    const raw = tokens?.ctx ?? "";
    const n = parseInt(raw, 10);
    return Number.isFinite(n) ? n : null;
  };

  return (
    <aside class="side">
      <div class="plate">Spaces</div>
      <Show when={p.tree} fallback={<div class="row micro">connecting…</div>}>
        <For each={(spaces() as any).top ?? []}>
          {(w) => (
            <>
              <div class={`row ${panesOf(w.workspace_id).some((x) => x.pane_id === p.selectedPane) ? "sel" : ""}`}
                   onClick={() => { const pane = panesOf(w.workspace_id).find((x) => x.focused) ?? panesOf(w.workspace_id)[0]; if (pane) p.onSelectPane(pane.pane_id); }}>
                <span class={`st ${statusClass(w.agent_status)}`} />
                <span class="nm ws">{w.label}</span>
                <span class="cnt">{w.tab_count}</span>
              </div>
            </>
          )}
        </For>
        <For each={(spaces() as any).groups ?? []}>
          {(g: { label: string; items: Tree["workspaces"] }) => (
            <>
              <div class="row"><span class="st" /><span class="nm ws">{g.label}</span><span class="cnt">{g.items.length}</span></div>
              <For each={g.items}>
                {(w) => (
                  <div class={`row l2 ${panesOf(w.workspace_id).some((x) => x.pane_id === p.selectedPane) ? "sel" : ""}`}
                       onClick={() => { const pane = panesOf(w.workspace_id).find((x) => x.focused) ?? panesOf(w.workspace_id)[0]; if (pane) p.onSelectPane(pane.pane_id); }}>
                    <span class={`st ${statusClass(w.agent_status)}`} />
                    <span class="nm wt">{w.worktree?.branch ?? w.label}</span>
                    <span class="tag">{w.agent_status === "blocked" ? "WAITING" : ""}</span>
                  </div>
                )}
              </For>
            </>
          )}
        </For>
      </Show>

      <div class="plate" style="margin-top:14px">Accounts</div>
      <Show when={p.accounts} fallback={<div class="row micro">cswap…</div>}>
        <For each={p.accounts!.accounts}>
          {(a, i) => {
            const u = usageOf(a);
            return (
              <div class={`bio ${a.active ? "" : "off"}`} onDblClick={() => !a.active && p.onSwitchAccount(a.number)}
                   title={a.active ? a.email : `${a.email} — double-click to switch`}>
                <div class="hd"><span class="num">{String(i() + 1).padStart(2, "0")}</span><span class="acc">{accountLabel(a)}</span>
                  <span class="tag">{a.active ? "ACTIVE" : "DBL-CLICK · SWITCH"}</span></div>
                <Quota label="5H" pct={u.five} />
                <Quota label="7D" pct={u.week} />
                <For each={u.scoped}>{(s) => <Quota label={s.name.slice(0, 3).toUpperCase()} pct={s.pct} />}</For>
                <Show when={a.active}>
                  <div class="cool">
                    <div><div class="ic">⟳</div><div class="t">{u.fiveReset ?? "--"}</div><div class="l">5H RESET</div></div>
                    <div><div class="ic">⟳</div><div class="t">{u.weekReset ?? "--"}</div><div class="l">7D RESET</div></div>
                    <For each={u.scoped}>{(s) => <div><div class="ic">◈</div><div class="t">{s.reset ?? "--"}</div><div class="l">{s.name.toUpperCase()}</div></div>}</For>
                  </div>
                </Show>
              </div>
            );
          }}
        </For>
      </Show>

      <div class="plate" style="margin-top:8px">Agents <span class="tag lc">by recency</span></div>
      <For each={agents()}>
        {(a) => (
          <div class={`row ${a.pane_id === p.selectedPane ? "sel" : ""}`} onClick={() => p.onSelectPane(a.pane_id)}>
            <span class={`st ${statusClass(a.agent_status)}`} />
            <span class="nm">{a.name ?? a.display_agent ?? a.agent ?? a.pane_id}</span>
            <Show when={ctxOf(a.tokens) !== null}>
              <span class="seg small"><i style={{ width: `${ctxOf(a.tokens)}%` }} /></span>
              <span class="pc">{ctxOf(a.tokens)}</span>
            </Show>
          </div>
        )}
      </For>

      <SessionMap tree={p.tree} selectedPane={p.selectedPane} onSelectPane={p.onSelectPane} />
    </aside>
  );
}

function Quota(p: { label: string; pct: number | null }) {
  const hot = () => (p.pct ?? 0) >= 80;
  return (
    <div class="qrow">
      <span class="lb">{p.label}</span>
      <span class={`seg ${hot() ? "hot" : ""}`}><i style={{ width: `${p.pct ?? 0}%` }} /></span>
      <span class={`pc ${hot() ? "hot" : ""}`}>{p.pct ?? "--"}</span>
    </div>
  );
}

function SessionMap(p: { tree?: Tree; selectedPane: string | null; onSelectPane: (id: string) => void }) {
  const cells = () => {
    const t = p.tree;
    if (!t) return [];
    return t.tabs.map((tab) => {
      const panes = t.panes.filter((x) => x.tab_id === tab.tab_id);
      const agent = panes.find((x) => x.agent);
      const ws = t.workspaces.find((w) => w.workspace_id === tab.workspace_id);
      return { tab, pane: agent ?? panes[0], status: tab.agent_status, title: `${ws?.label ?? ""} / ${tab.label}` };
    });
  };
  return (
    <div class="map">
      <div class="hd"><span>SESSION MAP</span>
        <span class="micro">{p.tree?.workspaces.length ?? 0} SPACES · {p.tree?.tabs.length ?? 0} TABS · {p.tree?.agents.length ?? 0} AGENTS</span></div>
      <div class="cells">
        <For each={cells()}>
          {(c) => (
            <b class={`${c.pane?.agent ? statusClass(c.status) || "i" : ""} ${c.pane?.pane_id === p.selectedPane ? "cur" : ""}`}
               title={c.title} onClick={() => c.pane && p.onSelectPane(c.pane.pane_id)} />
          )}
        </For>
      </div>
      <div class="lg"><span class="r">■ WORKING</span><span class="o">■ WAITING</span><span class="t">■ IDLE</span><span>□ SHELL</span></div>
    </div>
  );
}
