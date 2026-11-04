import { For, Show, createMemo, createSignal } from "solid-js";
import { accountLabel, usageOf, type AgentStatus, type CswapAccount, type CswapList, type Pane, type Tab, type Tree, type Workspace } from "../api";
import {
  buildTree, ctxPct, defaultOpen, firstPane, loadOpen, saveOpen, waiting,
  type RepoNode, type RootNode, type SessionNode, type SpaceNode, type TabNode,
} from "../tree";

export interface SwitchError { number: number; message: string }

interface Props {
  tree?: Tree;
  accounts?: CswapList;
  /** Why `cswap list` failed; shown in the Accounts block, the rest of the UI stays alive. */
  accountsError?: string | null;
  /** Account number of the `cswap switch` in flight, null/undefined when idle. */
  switching?: number | null;
  switchError?: SwitchError | null;
  selectedPane: string | null;
  onSelectPane: (paneId: string) => void;
  onSwitchAccount: (number: number) => void;
  onRefreshAccounts: () => void;
  /** Right-click menus; App owns the actions. */
  onWorkspaceMenu: (e: MouseEvent, w: Workspace) => void;
  onTabMenu: (e: MouseEvent, t: Tab) => void;
  onPaneMenu: (e: MouseEvent, pane: Pane) => void;
  onAccountMenu: (e: MouseEvent, a: CswapAccount) => void;
  /** Create at any level, like Herdr: space (optionally cloned from one), tab, split. */
  onNewSpace: (sourceWorkspaceId?: string) => void;
  onNewTab: (workspaceId: string) => void;
  onSplit: (paneId: string) => void;
}

type Node = RootNode | TabNode;

const statusClass = (s: AgentStatus) => (s === "working" ? "w" : s === "blocked" ? "b" : s === "done" ? "d" : "");
// Only the words the dot cannot carry: working/idle are the dot alone.
const statusWord = (s: AgentStatus, shell: boolean) =>
  shell ? "SHELL" : s === "blocked" ? "WAITING" : s === "done" ? "DONE" : "";
const storage = () => (typeof localStorage === "undefined" ? undefined : localStorage);

/**
 * Sidebar (M6): one tree — repo › space › tab › session — instead of the old
 * Spaces list plus a status-sorted Agents list. Herdr's server order is kept at
 * every level; urgency is a pinned WAITING strip above the tree, not a sort.
 */
export default function Sidebar(p: Props) {
  const roots = createMemo(() => buildTree(p.tree));
  const waits = createMemo(() => waiting(roots()));

  // Manual open/closed overrides; everything else follows `defaultOpen`.
  const [open, setOpen] = createSignal<Record<string, boolean>>(loadOpen(storage()));
  const isOpen = (n: Node) => open()[n.id] ?? defaultOpen(n, p.selectedPane);
  const toggle = (n: Node, e: MouseEvent) => {
    e.stopPropagation();
    const next = { ...open(), [n.id]: !isOpen(n) };
    setOpen(next);
    saveOpen(storage(), next);
  };
  const select = (n: Node) => { const pane = firstPane(n); if (pane) p.onSelectPane(pane.pane_id); };
  const stop = (e: MouseEvent, fn: () => void) => { e.stopPropagation(); fn(); };

  const Caret = (q: { node: Node; color: string }) => (
    <button class={`car ${q.color}`} title={isOpen(q.node) ? "collapse" : "expand"} onClick={(e) => toggle(q.node, e)}>
      {isOpen(q.node) ? "▾" : "▸"}
    </button>
  );

  const SessionRow = (q: { s: SessionNode; level: "tab" | "pane"; tab: Tab }) => {
    const ctx = () => ctxPct(q.s.agent?.tokens ?? q.s.pane.tokens);
    const shell = !q.s.agent && !q.s.pane.agent;
    return (
      <div class={`row ${q.level} ${q.s.id === p.selectedPane ? "sel" : ""}`}
           title={q.s.pane.cwd ?? undefined}
           onClick={() => p.onSelectPane(q.s.id)}
           onContextMenu={(e) => { e.preventDefault(); q.level === "tab" ? p.onTabMenu(e, q.tab) : p.onPaneMenu(e, q.s.pane); }}>
        <span class="car sp" />
        <span class={`st ${statusClass(q.s.status)}`} />
        <span class={`nm ${shell ? "sh" : ""}`}>{q.level === "tab" ? q.tab.label : q.s.label}</span>
        <span class="kind">{q.s.agentKind ? q.s.agentKind.toUpperCase() : "—"}</span>
        <Show when={ctx() !== null}>
          <span class="seg small"><i style={{ width: `${ctx()}%` }} /></span>
          <span class="pc">{ctx()}</span>
        </Show>
        <span class={`tag ${statusClass(q.s.status)}`}>{statusWord(q.s.status, shell)}</span>
        <button class="add" title="split (pane.split)" onClick={(e) => stop(e, () => p.onSplit(q.s.id))}>+</button>
      </div>
    );
  };

  const TabRow = (q: { t: TabNode }) => (
    <Show when={!q.t.single} fallback={<SessionRow s={q.t.single!} level="tab" tab={q.t.tab} />}>
      <div class={`row tab ${!isOpen(q.t) && q.t.sessions.some((s) => s.id === p.selectedPane) ? "sel" : ""}`}
           onClick={() => select(q.t)} onContextMenu={(e) => { e.preventDefault(); p.onTabMenu(e, q.t.tab); }}>
        <Caret node={q.t} color="cy" />
        <span class={`st ${statusClass(q.t.status)}`} />
        <span class="nm">{q.t.tab.label}</span>
        <button class="add" title="split (pane.split)" onClick={(e) => stop(e, () => { const pane = firstPane(q.t); if (pane) p.onSplit(pane.pane_id); })}>+</button>
        <span class="cnt">{q.t.sessions.length}</span>
      </div>
      <Show when={isOpen(q.t)}>
        <div class="kids t"><For each={q.t.sessions}>{(s) => <SessionRow s={s} level="pane" tab={q.t.tab} />}</For></div>
      </Show>
    </Show>
  );

  const SpaceRow = (q: { s: SpaceNode }) => (
    <>
      <div class={`row space ${!isOpen(q.s) && q.s.tabs.some((t) => t.sessions.some((x) => x.id === p.selectedPane)) ? "sel" : ""}`}
           title={q.s.workspace.worktree?.checkout_path ?? undefined}
           onClick={() => select(q.s)} onContextMenu={(e) => { e.preventDefault(); p.onWorkspaceMenu(e, q.s.workspace); }}>
        <Caret node={q.s} color="red" />
        <span class={`st ${statusClass(q.s.status)}`} />
        <span class="nm">{q.s.workspace.worktree?.branch ?? q.s.workspace.label}</span>
        <Show when={q.s.root}><span class="badge root" title="primary checkout">root</span></Show>
        <Show when={q.s.linked}><span class="badge" title="linked worktree">⎇</span></Show>
        <button class="add" title="new tab (tab.create)" onClick={(e) => stop(e, () => p.onNewTab(q.s.id))}>+</button>
        <span class="cnt">{q.s.tabs.length}</span>
      </div>
      <Show when={isOpen(q.s)}>
        <div class="kids w"><For each={q.s.tabs}>{(t) => <TabRow t={t} />}</For></div>
      </Show>
    </>
  );

  const RepoRow = (q: { r: RepoNode }) => (
    <>
      <div class="row repo" onClick={(e) => toggle(q.r, e)}>
        <Caret node={q.r} color="yel" />
        <span class="nm">{q.r.label}</span>
        <button class="add" title="new space in this repo (workspace.create)"
                onClick={(e) => stop(e, () => p.onNewSpace((q.r.spaces.find((s) => s.root) ?? q.r.spaces[0])?.id))}>+</button>
        <span class="cnt">{q.r.spaces.length} · {q.r.agents}</span>
      </div>
      <Show when={isOpen(q.r)}>
        <div class="kids r"><For each={q.r.spaces}>{(s) => <SpaceRow s={s} />}</For></div>
      </Show>
    </>
  );

  return (
    <aside class="side">
      <div class="plate-row"><div class="plate">Sessions</div><button class="plus" title="new space (workspace.create)" onClick={() => p.onNewSpace()}>+</button></div>

      <Show when={waits().length > 0}>
        <div class="pin">
          <div class="pl"><span>WAITING · {waits().length}</span><span>F2 ↵</span></div>
          <For each={waits()}>
            {(w) => {
              const i = w.path.lastIndexOf(" / ");
              return (
                <div class={`row ${w.session.id === p.selectedPane ? "sel" : ""}`} onClick={() => p.onSelectPane(w.session.id)}
                     onContextMenu={(e) => { e.preventDefault(); p.onPaneMenu(e, w.session.pane); }}>
                  <span class="st b" />
                  <span class="nm"><span class="path">{i >= 0 ? w.path.slice(0, i + 3) : ""}</span>{i >= 0 ? w.path.slice(i + 3) : w.path}</span>
                  <span class="kind">{w.session.agentKind?.toUpperCase() ?? ""}</span>
                </div>
              );
            }}
          </For>
        </div>
      </Show>

      <Show when={p.tree} fallback={<div class="row micro">connecting…</div>}>
        <div class="tree">
          <For each={roots()}>{(r) => (r.kind === "repo" ? <RepoRow r={r} /> : <SpaceRow s={r} />)}</For>
        </div>
      </Show>

      <div class="plate" style="margin-top:14px">Accounts</div>
      <Show when={p.accountsError}>
        {(e) => <div class="row micro" style="color:var(--red)" title={e()}>cswap недоступен: {e()}</div>}
      </Show>
      <Show when={p.accounts} fallback={<Show when={!p.accountsError}><div class="row micro">cswap…</div></Show>}>
        {(acc) => (
          <For each={acc().accounts}>
            {(a, i) => {
              const u = usageOf(a);
              const busy = () => p.switching === a.number;
              const failed = () => { const e = p.switchError; return e && e.number === a.number ? e.message : null; };
              const canSwitch = () => !a.active && p.switching == null;
              return (
                <div class={`bio ${a.active ? "" : "off"}`} onDblClick={() => canSwitch() && p.onSwitchAccount(a.number)}
                     onContextMenu={(e) => { e.preventDefault(); p.onAccountMenu(e, a); }}
                     title={a.active ? a.email : `${a.email} — double-click to switch`}>
                  <div class="hd"><span class="num">{String(i() + 1).padStart(2, "0")}</span><span class="acc">{accountLabel(a)}</span>
                    <span class="tag">{a.active ? "ACTIVE" : busy() ? "SWITCHING…" : "DBL-CLICK · SWITCH"}</span></div>
                  <Show when={failed()}>{(m) => <div class="micro" style="color:var(--red)" title={m()}>switch failed: {m()}</div>}</Show>
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
        )}
      </Show>

      <SessionMap tree={p.tree} repos={roots().filter((r) => r.kind === "repo").length} selectedPane={p.selectedPane} onSelectPane={p.onSelectPane} />
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

function SessionMap(p: { tree?: Tree; repos: number; selectedPane: string | null; onSelectPane: (id: string) => void }) {
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
        <span class="micro">{p.repos} REPOS · {p.tree?.workspaces.length ?? 0} SPACES · {p.tree?.tabs.length ?? 0} TABS · {p.tree?.agents.length ?? 0} AGENTS</span></div>
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
